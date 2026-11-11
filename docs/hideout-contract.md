# Hideout module contract, v1

Status: spec, contract version **1.11** (v1.10 plus a catalog entry's optional `min_hub_release`,
the Hideout release Update Hideout links, #991; v1.10 was v1.9 plus the manifest's `[limits]` and the rules of
the activation events `open`, `fact:`, `app:` and `device:`, #983; v1.9 was v1.8 plus the state
`revoked` and its card's fact `module.<id>.check`, #1026; v1.8 was v1.7 plus the last spec items of #918: the canvas-shape
and table-row caps, which stream overs are refused and which closed, when the backoff run resets,
and the page caps in the schema; v1.7 was v1.6 plus the routed `ui.asset.put` answer and the app
watch's `apps.running.<name>`, #991; additive, no breaking change; every v1.1 addition is marked
"(v1.1)", every v1.2 one "(v1.2)", every v1.3 one "(v1.3)", every v1.4 one "(v1.4)", every v1.5
one "(v1.5)", every v1.6 one "(v1.6)", every v1.7 one "(v1.7)", every v1.8 one "(v1.8)", every
v1.9 one "(v1.9)", every v1.10 one "(v1.10)", every v1.11 one "(v1.11)"; v1.1 was v1.0 plus the decisions of #918), originally **1.0** (#905), hub-first design (re-scoped 11/09). Nothing in it
is built yet beyond what Foxglove's plugin system already does. Rationale lives in the Hideout
design, `review/hideout-modular-design.md` ("the design"); this spec links to it by section and
does not repeat it. Test vectors: `tests/hideout-vectors/` (#906).

**Changelog.**
- **1.11** (#991): the Hideout release a module version needs. A catalog entry may carry the
  optional `min_hub_release` `{version, url}`: the first Hideout release that serves this
  module version, and that release's page (§12.4). **Update Hideout** on a Needs a newer
  Hideout card and Modules row opens that page when the installed version's entry in its own
  source's accepted catalog has one and its `url` is on the allow-list (`https`, host
  `github.com`, path `/BanditF/foxglove/releases` or below; no query, fragment, escape or `..`);
  anything else opens the releases page, as before (§7.8). A field that can't be read is
  ignored, never a catalog error, so it can't cost a catalog its revocations. The manifest gets
  no key: the hub a module needs a newer one than reads only its head (`id`, `name`,
  `version`, `hub`, §4.1), never a key added later, so only the catalog can say which release. Vectors: `trust/hub-release.json`; schema
  type `release.HubRelease`. One optional catalog field, which an older reader ignores, no
  message and no manifest key: `hub` stays 1.2 (§8.7).
- **1.10, clarifications** (#983): the supervisor's last states, made exact. **Waiting** (§4.4, §7.8): a running
  module whose `requires` or `channel.open:` provider is down reads `waiting` and names the
  root of the chain in the new fact `module.<id>.waiting` (an id); nothing is restarted or
  stopped for it, and it reads `running` again once the root is back. **needs-hub** is found at
  hub start or at a launch; nothing but Go back starts it. **quarantined** (§5.5) stops the
  module: it doesn't run while its panels are quarantined, and only Start again or Go back
  starts it. **Hot swap** (§7.6): Force sends `module.drain {reason: "force"}` and swaps
  whatever it answers; an error answer to a drain counts as done; the old instance has 5 s to
  exit after `module.exit` before its job is ended (not a crash); both `swap` modes stop the
  old instance before the new one starts for now; a crash while an update waits for Review
  restarts into the new version with the old grants and shows Update waiting for you again.
  **needs-hub is listed** (§4.1, §7.8, #1096): a reader checks `hub` before unknown keys, and a
  module whose manifest asks for a newer hub is listed from its head (`id`, `name`, `version`,
  `hub`) for every source, lab included, never dropped; installing such a version stays refused.
  Vector `manifests/invalid/hub-newer-minor-new-key.toml`.
  No message, manifest key or error id is new; it adds one hub fact and stays `hub` 1.2. Vectors: the supervisor
  transcripts `state-waiting`, `root-cause-once`, `state-needs-hub`, `state-quarantined`,
  `state-update-busy`, `state-update-waiting`, `state-updating` and
  `update-waiting-permission` now run.
- **1.10** (#983): resource limits and activation events. A manifest's new `[limits]` table
  (`memory_mb`, `cpu_percent`) sets the module's Job Object caps; the supervisor clamps each to
  the hub's maximum (4,096 MiB and 50 % today), and a module without it gets the maximum, as
  every module did before (§4.2, §7.7). A manifest using it says `hub = "1.2"`; a bad value is
  the new error id `manifest.limits` (§15). The activation events `open`, `fact:`, `app:` and
  `device:` get their rules (§4.6): a fact event starts the module when a matching fact turns
  true by the `when` rule (set and not `false`, `null`, `0` or `""`), or is already true when
  the module is armed (hub start, Turn on, Start normally); `app:<name>.exe` is the app watch's
  `apps.running.<name>` turning true; `open` is a page of the module coming on screen (a tab or
  a pop-out, its panels registered or not). An event for a module that is connected, off or
  held by safe mode starts nothing. Vectors: `manifests/valid/limits*.toml`,
  `manifests/invalid/limits-*.toml`; `manifests/invalid/hub-newer-minor.toml` now says `1.3`, and
  the hub's `initialize` results say `1.2`. One manifest key: `hub` is 1.2 (§8.7).
  Also (#983, #1092): the opt-in AppContainer (§7.7), with stdio modules, `secrets` and direct
  channels contained; `channel.create`'s optional `consumer_pid` and `consumer_sid`, the
  grant's optional `pid`, and the contained provider's pipe name (§8.4). Vectors:
  `channels/channel-wire.json`'s new pipe rows.
- **1.9** (#1026): a version its publisher withdrew. When an accepted catalog revokes the
  installed version of a module (§12.4), the supervisor stops it and publishes the new state id
  `revoked` (§7.8): at hub start (the shallow verify refuses the launch, §12.5), when a catalog
  is accepted, and at any launch. It reads "Withdrawn" in the UI, with "Its publisher withdrew
  this version, so Hideout stopped it." (not "for safety": Hideout doesn't know why) and the
  version in the card's title. Nothing starts it until another version is installed or gone back
  to; a Turn on is refused with `trust.revoked`; it stays enabled, so a fixed version starts by
  itself. The publisher's `reason` is drawn on the card only, as plain text, cleaned and capped
  at 160 characters (the full text in Copy diagnostics). One notification per withdrawn
  version, ever. New fact `module.<id>.check` (`checking`, `none`, `failed` or `null`): the
  card's **Check for updates**. `module.<id>.previous` names Go back's version for this state
  too, and `module.<id>.revoked` also clears when the installed version is no longer withdrawn.
  Vectors: `transcripts/supervisor/state-revoked.json`, `facts/supervisor-facts.json`. A new
  state id and one fact, no message, no manifest key: `hub` stays 1.1 (§8.7); a reader that
  doesn't know `revoked` shows it as a module that doesn't run.
- **1.8** (#918): the spec items #918 still held. Canvas shapes and table rows get numbers: at
  most 1,000 shapes in one `canvas` and 10,000 rows in one `table` (§5.2, §5.5; the numbers the
  crate already enforced, so no shipped panel changes result). The page caps are published in the
  schema as `limits` (`schema/hideout-v1.json`), as §5.5 always said they were: `nodes`, `depth`,
  `text_bytes`, `canvas_shapes`, `table_rows`, `streams`, `frame_bytes`, `frames_per_s` and
  `message_nesting`. A stream over a cap (§5.5): over the stream count is refused (the panel is
  `ui.limits` at `ui.register`, or the patch at `ui.patch`, and no channel opens); a frame over
  4 MiB or over 60 frames a second is closed (the hub closes that channel, shows "This preview
  couldn't be shown." and doesn't reopen it until the module's next instance). The backoff run
  (§7.7): a crash with no other crash in the ten minutes before it starts a new run at the 1 s
  step; Start again, Start with defaults and a rollback by itself start one too (what the
  supervisor already did). Vectors: `panels/valid/at-cap-*.json` and
  `panels/invalid/over-cap-*.json`, each page cap at the cap and one over (§16), and
  `transcripts/supervisor/backoff-resets-after-quiet.json`. The vectors generator #914 used was
  never committed and no copy survives; vectors are edited by hand (§16). One schema key, no
  message, no manifest key: `hub` stays 1.1 (§8.7).
- **1.7** (#991): two router leftovers. `ui.asset.put` is answered by the hub's panel host: the
  router hands the call over and sends the panel host's answer back to the module, `{}` or a
  refusal as error -32602 with the §15 id in `data.id` (`ui.limits`) and the reason; an answer
  that doesn't come in 30 s is error 4 (§5.3, §8.8). The app watch, `hideout.appwatch`, is the
  hub's own and watches only the apps a manifest asks about (its `when` clauses, `enriches`,
  `app:<name>.exe` activation and `apps.watch:<name>.exe` permissions); it publishes the new
  optional fact `apps.running.<name>` (`true` while the app runs, `false` while it doesn't) and
  `apps.seen.<name>`, now "has run on this PC", kept in `apps-seen.txt` (§2.3, §4.6). A module
  holding `apps.watch:<name>.exe` may read both facts of that app; `apps.*` facts are the hub's,
  never a module's (§4.6, §8.8). A `when` may name `apps.running.<name>` with the same one-label
  rule. Vectors: `facts/app-facts.json`, `manifests/invalid/when-apps-running-with-exe.toml`,
  `transcripts/ui/asset-put-refused.json`. No message, no manifest key: `hub` stays 1.1 (§8.7).
- **1.6** (#998): the seven pop-out spec gaps of the pop-outs plan
  ([hideout-popouts-ux.md](hideout-popouts-ux.md)), for hub windows (§2.5). Closing a pop-out
  (×, Ctrl+W, Alt+F4) docks the page back quietly: it is never `closed` and never asks; only
  Close page sends `closed` and asks about unsaved work (§2.5, §5.9). A pop-out on screen stays
  `visible` while the hub's main window is in the tray; only the main window's tabs go `hidden`
  (§5.9). Pop-outs are unowned top-level windows (§2.5). They come back at once after a hub
  restart or when the user opens Hideout; a start in the tray at sign-in waits for the user's
  first open (§2.5). Placement is kept per page and per screen layout, at most 4 layouts per
  page, a screen known by its display target path (§2.5), in a new hub-owned `hideout.toml`
  table, `[windows]` (§2.3, §7.3; crate `config::Windows`, schema, vectors `config/windows.*`)
  with the id `config.windows` (§15), which drops one page's entry and never the file. Docking
  by dragging a pop-out onto the tab strip is optional (MAY; it may ship later). Module windows
  keep the title, AUMID and placement rules themselves, under their `data\`; Keep on top
  is the hub's, for pop-outs only (§2.5, §5.7). A ↗ Pop out button for the active tab is named as
  one way to pop out (hub UI, no wire). No message, no manifest key: `hub` stays 1.1 (§8.7).
- **1.5** (#976): the nine stream spec gaps of the live streams plan
  ([hideout-streams-ux.md](hideout-streams-ux.md)), for panel streams (§5.4). Panels (§5.2): a
  `canvas` declares its unit size (`width`, `height`) and an `image` its `aspect` (and optionally
  `width`), so the hub lays the box out before the first frame; an `image` takes
  `scaling: "smooth" | "pixel"` (default `smooth`, an unknown word draws `smooth`); a `text` takes
  `mono`. Streams (§5.4): `markdown` takes a `stream` too; an optional `stale_after_ms` on a
  canvas or image stream is the gap after which the hub says "No new frames" (without it, never);
  a streamed `image` needs a `label`. Written down, not new wire: a `ui.patch` of a streamed
  node's `text` replaces everything shown and the stream keeps appending after it (§5.3, §5.4);
  stream-appended text has its own cap, 1 MiB per node, oldest whole lines dropped first (§5.5);
  the hub keeps a text node's text while hidden, sends `ui.visibility visible` before it reopens
  the panel's streams, and a module restores text appended while hidden by patching before it
  answers the stream's `channel.create` (§5.4, §5.9); a text frame MUST end on a character
  boundary, and one that isn't whole UTF-8 is dropped (§8.4, as v1.1 readers already did); the
  hub reopens a stream its provider closed up to three times (1, 2, 4 s), then shows "Stopped
  updating"; removing `stream` closes the channel cleanly; visibility stays per panel (§5.4).
  Informative, hub-only: with reduced motion the hub draws streams at 2 frames a second and shows
  a Live dot. Tightenings, none of which a shipped module hits (no module streamed before H2.9): a
  tile's stream cap is 0 (`ui.limits`, §5.5), and a size, `scaling`, `mono` or `stale_after_ms` of
  the wrong type, or a streamed image without a `label`, is `ui.prop` (§15). `hub` stays 1.1: no
  manifest key, only optional panel props, which an older hub ignores (§8.7).
- **1.4** (#975): the spec gaps S1–S11 of the module tiles and pages plan
  ([hideout-plugin-modules-ux.md](hideout-plugin-modules-ux.md)). Panels (§5.2): a `tone` on
  `value` and `label` (`neutral`, `active`, `live`, `ok`, `attention`, `off`; unknown draws
  `neutral`); a `button` may say where it `opens` (`page`, `page#<node id>`, `settings`) or which
  declared `action` it is, never both; a published icon list (§5.1), shipped with the schema. Tiles
  get small fixed caps (§5.5: 12 nodes, depth 3, one button, 64 bytes of text) and `ui.visibility`
  (§5.9). The hub draws a Keys section on every page of a module with `[[actions]]` (§4.7). A
  `file` setting kind with an optional `filter` (§4.7) is the one manifest addition, so **`hub`
  becomes 1.1** (§8.7): a manifest using it says `hub = "1.1"`; every `hub = "1.0"` manifest is
  still served. The optional fact `apps.seen.<name>` (§4.6). Written down, not new wire: an
  outside app that is missing or a busy port is a module state, not a crash (§7.7); legacy plugins
  declare panels in a `module.toml` that keeps `transport = "stdio"`, and the stdio bridge carries
  `ui.*` both ways (§4.10, §8.2). New ids `ui.prop` and `ui.action` (§15). The one tightening:
  tile trees over the tile caps are now `ui.limits`; no module shipped a tile before this.
  Also (#982): the supervisor's facts `module.<id>.restart` and `module.<id>.recovery` (§7.8).
  Also (#986), hub-side only: how a restarted hub takes modules back (§7.9). A record in
  `run\instances.json` names a process by pid and start time; a survivor goes into a fresh Job
  Object; one supervisor per hub folder.
  Also (#977), written down, not new messages: the direct channel's wire (§8.4). The nonce is
  the consumer's first frame, at most 256 bytes, compared byte for byte; a frame is at most
  64 MiB; the pipe takes one client ever and the provider waits 30 s for it; a channel pipe is
  `\\.\pipe\hideout-ch-<rest>`, and the hub refuses any other `{pipe}` with error 6, `data.id`
  `channel.pipe` (new id in §15). Vectors: `channels/channel-wire.json`,
  `transcripts/stream/channel-create-bad-pipe.json`.
  Also (#959, #985), additive: a catalog revoking a running module's version ends the leases it
  holds with `lease.closed {reason: "revoked"}` (§8.5), and the hub fact `module.<id>.revoked`
  carries the catalog's reason (§7.8). A legacy `plugin.toml`'s `api` is checked against the
  Foxglove plugin API version, not `hub` (§4.10). Vector:
  `transcripts/lifecycle/lease-closed-revoked.json`.
- **1.3, added** (#959): router gaps the first router build found (#958). Lease end (§8.5, §8.8): `lease.closed {lease, reason}` (H → M) tells a provider that a lease it handed out ended, and a provider event with `to: <lease>` goes only to that lease's holder. Who is an MCP client, who is the first-party `hideout` CLI, and who runs the `initialize` pid and exe-hash check are now written down (§8.2). `hub` stays 1.0: one optional notification and one optional event field, which a peer that does not know them ignores.
- **1.3** (#953): open page panels are tabs (§5.9). Two optional notifications: `ui.badge` (M → H) sets a tab's badge (`none`, `attention`, `dirty` or `count`), and `ui.visibility` (H → M) tells a module when its panel is `visible`, `hidden` or `closed`, so it can stop work the user can't see. A tab uses the panel's existing `title` and `icon`. Pop-outs dock back to their tab (§2.5). `hub` stays 1.0: both are optional notifications, and a peer that does not know one drops it (JSON-RPC), so nothing has to be negotiated.
- **1.2** (#918): two wire rules the transcript vectors found missing (§8.1). A request id that is still in flight on the same connection answers error -32600 `rpc.duplicate-request-id` with id null, and the first call is unaffected. A line over 256 KiB answers -32600 `rpc.frame-too-large` with id null, then the hub closes that channel; the supervisor counts it as a crash (§7.7). Also written down, not new behaviour: a result arriving after the caller's error 4 is dropped. New ids in §15. The trust vectors' `seen_after` is keyed by source id, as §7.3 has it, not by family.
- **1.1.1** (#941): a message nests at most 128 deep (arrays and objects, counted before parsing; was 4096, which overflowed the stack). Over it is `ui.limits`, the whole message refused. Every reader MUST check the depth before it recurses, or parse without recursion. The same cap holds for brackets in a TOML file (`module.toml`, legacy manifests, `hideout.toml`): over it is `manifest.syntax`. Not a wire change for any message that was within the panel caps.
- **1.1** (#918): closes nine gaps the test vectors found: panel caps (§5.5), `width_or_count` of
  text frames (§8.4), `[[ui.windows]]` and MCP `method` checks (§4.7, §9), the never-list ids
  for `facts.write` (§6.6), supervisor backoff steps (§7.7), developer mode (§12.6), the legacy
  root's scope (§12.2), user-anchored catalogs without `family` (§12.4), `hideout.toml` checks
  (§13.4, §13.6); new ids in §15.
- **1.0** (#905): first version.

**What Hideout is.** One resident hub, `hideout.exe`, and many modules, each its own process. The
hub draws the window and the tray, keeps the registry, routes control messages and restarts
modules. Every feature (the keyboard link, lights, scenes, screens, art, overlay, keymap editor,
integrations, Vulpes) is a module. Design §0, §3.

**Sources this spec grows out of.** Foxglove's plugin system ([local-api.md](local-api.md),
`crates/plugin-api`, `crates/plugin-trust`) and Arete's component installer
(`src/arete/components.py`, `footprint.py`, `selfupdate.py`). Where they agree with this spec,
their code is the reference.

**Words.**
- MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119.
- **Hub:** the `hideout.exe` process.
- **Module:** an installable unit with a `module.toml` (§4). A running one is an **instance**.
- **Panel:** UI a module declares and the hub draws (§5).
- **Reader:** any program that parses manifests, catalogs or the inventory: the hub, the
  `hideout` CLI, the `hideout-core` crate, Vulpes's pydantic models.
- **Outside tool:** a client of the hub's pipe that the hub did not start: scripts, Stream Deck,
  `foxglove-cli`, the `hideout` CLI, an MCP adapter.
- **Module source:** a configured place modules come from (§13).
- **Shared output:** anything that leaves the PC or is copied for someone else: Copy diagnostics,
  crash reports, health signals, telemetry (§13.5).

## 1. Decisions this spec follows

User decisions of 11/09 (design §9).

| Pick | Decision | Where |
|---|---|---|
| A | A new thin `hideout.exe`, built from Foxglove's shell code | §2 |
| B | Live panels in the hub with streams; only wizards and editors open their own window | §5.4, §5.7 |
| C | Vulpes gets a native UI in the hub that can pop out; the browser UI is for the phone and the tailnet only | §5.8 |
| D | LampArrays: re-register the package with the Hideout Applications on a go | §4.8 (`[identity]`) |
| E | One sparse package, one Application per exe that needs identity | §4.8 |
| F | Foxglove ships as 0.3.0 (app-main's version is right; main's `app/Cargo.toml` is bumped from 0.2.0 to match); the Hideout cut-over follows | §1 (no rule depends on the number) |
| G | Opt-in preview on the user's PC from phase 3, with the one-owner rule | §8.6 |
| H | "Modules" and "panels" in the UI; the keyboard's left screen is the "Info screen" | §14 |

Kept from the first scope:
- **One Hideout root key** delegates a catalog key per family (§12).
- **Arete becomes Vulpes** in phase 6. `arete` stays an alias; `C:\AI` is unchanged (§4.3).
- **Lagopus, Zerda and Velox** are module profiles (editions) of one Vulpes build (§4.9).

User requirement of 11/09: **public and private modules** (§13).

Superseded by hub-first (do not reintroduce):
- "Library + CLI, no daemon." The hub is the resident process and the supervisor.
- "Foxglove features are built-in modules." Every feature is its own process.
- "Foxglove's tray gets a Vulpes section." The hub owns the one tray.
- Per-core pipes, run files and cross-core grants. There is one router pipe (§8).

## 2. The hub

### 2.1 Six jobs

The hub MUST do these six things and nothing else (design §3):

1. **Tray icon:** the glyph and its states; a menu of the hub's items plus the modules' declared
   actions; the tray panel, which hosts `tile` panels.
2. **Window frame:** the sidebar, the status strip, the Modules page; placement and DPI; theme
   tokens; the command palette (§2.6).
3. **Module registry:** what is installed, enabled and running, at which version and instance;
   the shallow trust check before every launch (§7.4); permission grants.
4. **Panel host:** draws declared panels (§5) and sends the user's input back to the owner.
5. **IPC router:** the pipe (§8.2), the routing table, the last-value facts cache, a permission
   check on every call, channel grants (§8.4).
6. **Light supervisor:** starts, contains and restarts modules (§7.7).

### 2.2 Not in the hub

The hub MUST NOT contain:
- device code, keyboard protocols, lighting, scenes or AI;
- module installers or updaters (they live in `hideout-core`, used by the `hideout.modules`
  module and the CLI);
- settings values (each module keeps its own; the hub draws the form);
- health checks, heartbeats or timers on modules;
- vendor knowledge;
- **a hard-coded list of module ids** (§13.3). What the hub shows comes from manifests, the
  registry and facts.

### 2.3 The hub's files

```
%LOCALAPPDATA%\Hideout\
  hideout.toml            the hub's own preferences (§7.3); written only by the hub;
                          (v1.6) its [windows] table keeps pop-out placement (§2.5)
  inventory.json          machine state: install records and grants (§7.3); written only by hideout-core
  modules\<id>\<version>\ an installed version; manifest at the top
  modules\<id>\data\      the module's own state; writable; not hashed
  modules\<id>\.staging\  an install or update being staged
  cache\panels\<id>\      last panel trees and facts (§2.4)
  catalogs\<source>\      last accepted root, catalogs and signatures, per source (§12, §13)
  run\instances.json      live instance tokens, for taking modules back after a hub restart (§7.9)
  run\hub-run.json        the running hub's mark and its last 3 unclean closes (§7.10)
  apps-seen.txt           (v1.7) the apps the app watch has seen run, one label a line (§4.6);
                          Settings' Forget seen apps clears it and the running watch's list
  logs\                   hub and module logs (stderr captured per module)
%APPDATA%\Hideout\modules\<id>\settings\   the module's settings folder, assigned by the hub
```

- (v1.6) Window placement: a pop-out's (popped out or not, Keep on top, its places per screen
  layout) is `hideout.toml`'s `[windows]` table (§2.5, §7.3); the main window's is the hub's own
  part of the same file. A module window's placement is the module's, under its `data\`
  (§2.5).
- **`catalogs\<source>\` (clarification, #1049).** One file per source,
  `catalogs\<source>\accepted.json`: the last catalog accepted from that source, byte for byte,
  with its signature, its family, epoch, sequence and expiry, and when this hub accepted it (UTC).
  For a root-anchored source it also holds the `root.json` (and signature) that catalog was
  verified against, never another one: when a newer root is accepted, every saved root-anchored
  catalog is verified again under it and saved with it, or no longer used. The file is replaced
  in one atomic rename, written before `seen` is raised (§12.4), and verified again in full at
  every hub start. Its sequence and accepted time go to Copy diagnostics only once verified.
- `HIDEOUT_HOME`, when set, replaces `%LOCALAPPDATA%\Hideout` and `%APPDATA%\Hideout` (tests).
- Every file and pipe here MUST have a current-user-only ACL.
- (v1.10, #983) One exception, only while modules run in AppContainers (§7.7): the router pipe
  also lets each such container's SID in (client rights, no create-instance) and carries a Low
  mandatory label, which Windows has one of per object, not per SID. A Low-integrity process of
  the user's then passes Windows' check too; the router refuses it (below Medium it admits only
  the hub's own containers, and never a client whose process or token it can't read), a module
  talks only to the pipe server whose pid is `HIDEOUT_HUB_PID`, and it opens the pipe at
  identification level (`SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`), so a server can't act
  as it. What stays open is in §7.7. (#1092) A channel pipe (§8.4) has the same exception for
  one consumer only: its own container's SID, with client rights and the Low label, only when
  the consumer runs in an AppContainer.
- A module's files outside `data\` and `settings\` are written only by `hideout-core`.

### 2.4 Draw first (UX2)

- The hub MUST draw every page and tile before any module answers.
- It persists each module's last panel trees and last facts under `cache\panels\<id>\`, keyed by
  module id and version. A version change discards the cache.
- A cold panel draws from the cache at once, with "Starting…" in its header and its controls
  disabled until `module.ready`. Then the live tree replaces it in place.
- **Optimistic input:** toggles, choices and sliders change at once in the hub and send
  `ui.event`. If the module refuses, the hub restores the old value and shows the reason inline.
  A node with `confirm: true` opts out (for example Flash).
- Slider drags send at most ~20 events a second, and always the final value.
- The hub keeps its egui context warm while hidden.
- **Budgets** (lab cases in phase 2): tray panel open < 100 ms; page switch < 1 frame from cache;
  input to visible change < 16 ms locally, < 100 ms confirmed.

### 2.5 Windows: pop-outs and module windows (UX19)

UX, states and wording (v1.6): [hideout-popouts-ux.md](hideout-popouts-ux.md).

Two kinds of window, one rule set:
- **Pop-out:** any page drawn by the hub in a second viewport of the hub process (§5.7).
- **Module window:** a top-level window a module draws itself (editors and wizards, §5.7).

Both MUST:
- use the title form `Hideout · <page or window name>`;
- share Hideout's AUMID, so they group under one taskbar button;
- remember their placement per screen layout (v1.6; was: per monitor), by the rules below;
- ask on close only if work would be lost. (v1.6) For a pop-out that is never: closing it docks
  the page back with its view state, so nothing is lost. Only Close page and Quit Hideout ask
  (§5.9).

Pop-outs also:
- (v1.6) are **unowned top-level windows** of the hub process. No pop-out is owned by the main
  window, so none minimises, hides or closes with it: with the main window in the tray, pop-outs
  stay open and live (`visible`, §5.9). Quit Hideout closes every window, after one ask that
  lists the pages with unsaved work.
- open from a tab's menu (Pop out), Ctrl+Shift+O, (v1.6) a ↗ **Pop out** button at the tab
  strip's right end for the active tab, the command palette, or dragging a tab out of the window.
  Popping out leaves a placeholder tab; a page is in one place at a time.
- go back to their tab when closed (v1.3; was: the sidebar). (v1.6) Spelt out:
  - **×** (and Ctrl+W, Alt+F4): the page docks back **quietly**, at its placeholder's place, as
    a tab that isn't activated. The main window is left as it is (in the tray stays in the tray,
    behind stays behind). Not `closed`, no ask.
  - **Dock back** (title row, More, window menu, Ctrl+Shift+O): the page lands at its
    placeholder's place as the active tab, and the main window is shown and brought forward.
  - **Close page** (More): the page closes as its tab would: `closed` (§5.9), after the ask if
    its badge is `dirty`.
- (v1.6, optional) MAY dock back when the user drags the window by its title row onto the main
  window's tab strip, landing where the drop shows. This needs the hub to see one of its windows
  move over another, which a viewport toolkit may not report; a hub without it leaves the drag
  out (it may ship later), and Dock back and × are the required ways. A drop on another pop-out
  does nothing (no tab groups).
- reopen at their places (v1.6, when):
  - after a hub restart (a crash, a GPU reset, §7.9): every page saved as popped out comes back
    in its window at once, drawn from the cache with "Starting…" (§2.4);
  - when the user opens Hideout (Start menu, tray): every saved pop-out comes back with it;
  - when the hub starts in the tray at sign-in: pop-outs do **not** open. They wait as
    placeholder tabs (`hidden`, §5.9) and come back the first time the user opens Hideout.
- offer "Keep on top", off by default, remembered per page (v1.6). It never puts the window above
  an exclusive full-screen app and never takes focus by itself.

Module windows (v1.6) follow the title, AUMID and placement rules, but the module keeps them: it
owns its window, saves its places by the rules below under its own `data\`, and restores
them itself. The hub never moves another process's window and adds no Keep on top to it; a module
MAY offer its own. The overlay (§5.7) is topmost by nature and keeps no placement beyond its own.

**Placement (v1.6).**
- **Screen.** A screen is known by its Windows display target path
  (`DISPLAYCONFIG_TARGET_DEVICE_NAME.monitorDevicePath`), compared without case. It carries the
  monitor's EDID maker and product code and its connector, so it stays the same when the screen
  is unplugged and plugged back into the same port. When no connected screen has a saved path, a
  connected screen whose EDID id (the path's second `#` part) matches, and is the only one that
  does, counts as the same screen (the same monitor on another port).
- **Screen layout.** The set of connected screens, each with its position and size in the
  virtual desktop and its scaling. Its id is the first 16 lowercase hex digits of the SHA-256 of
  one line per screen, `<path> <x> <y> <width> <height> <dpi>` (path lowercased; physical
  pixels; effective DPI), sorted, joined by `\n`. Only the hub computes and compares it.
- **Kept:** per page, popped out or not, Keep on top, and up to **4 places**, one per screen
  layout. A place is the layout, the screen, the position from that screen's top-left and the
  size, both in device-independent units (per-monitor DPI v2), the state (`normal`, `maximized`,
  or `snapped` with the snapped rectangle) and when it was saved. A fifth layout drops the place
  saved longest ago.
- **Saved** when the window stops moving or resizing and when it closes; never during a drag,
  never on a timer.
- **Restoring:** the place for the current layout; else the newest place on a screen that is
  connected now (from another layout); else the main window's screen, offset 32 px, the same size
  in device-independent units, clamped fully on screen. The title row is always on a screen. A
  pop-out is at least 360 × 240.
- **Screen unplugged while open:** Windows moves the window; the hub saves a place for the new
  layout and keeps the old one for when the screen is back.
- **File:** `hideout.toml`'s `[windows]` table, one entry per page, keyed `<module id>/<panel
  id>` (schema `config.Windows`, vectors `config/windows.*`):

```toml
[windows."example.chat/chat"]
popped_out = true
keep_on_top = false

[[windows."example.chat/chat".places]]
layout = "0123456789abcdef"
screen = '\\?\DISPLAY#TST0001#5&1a2b3c&0&UID1#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}'
x = 32.0          # from the screen's top-left, device-independent units
y = -8.0
width = 640.0     # above 0
height = 480.0
state = "normal"  # normal | maximized | snapped
saved = "2026-11-09T12:00:00Z"
```

  A reader checks each entry: a key that isn't `<module id>/<panel id>` (§4.3, §5.1), more than
  4 places or two for one layout, a `layout` that isn't 16 lowercase hex digits, an empty
  `screen`, a position that isn't a finite number, a size not above 0, an unknown `state` or key,
  or a `saved` not in the form `YYYY-MM-DDTHH:MM:SSZ` is `config.windows` (§15). The hub then
  drops that page's entry (it opens with the fallback place) and logs; it never refuses
  `hideout.toml` or stops for it, and the sources reader (§13.4) passes over `[windows]`.

### 2.6 Command palette

- Ctrl+K in the hub window opens a palette of every enabled module's declared `[[actions]]` and
  `[[ui.panels]]` pages whose `when` is true (§4.6).
- It is built from manifests and the registry only. It MUST NOT start a module to list it.

## 3. Module kinds

Every module has exactly one kind. All code runs out of process; there are **no in-process
plug-ins** (no dylibs, no scripts in the hub).

| Kind | What it is | Has `run` | Started by |
|---|---|---|---|
| `service` | A process in any language. It connects to the router and is supervised by the hub. | yes | the hub, on activation (§4.6) |
| `runtime` | A pinned third-party program a service launches. No API of its own. | no | the service that owns it |
| `content` | Data only: board profiles, catalogs, art, agent packs, editions. | no | never runs |
| `firmware` | Digitalis images, under `crates/flash`'s rules. Installed and verified, never enabled or started. | no | never runs on the PC |
| `adopted` | An install the hub registers but did not make (Arete today). Checked by path and version only. | yes (absolute `run[0]`) | the hub, on activation |

- There is no `core` and no `builtin` kind.
- Only `service` and `adopted` modules are processes the hub starts and stops.
- A `runtime`'s processes belong to the service that launched them, inside that service's Job
  Object (§7.7).

## 4. The manifest: `module.toml`

### 4.1 File and syntax

- A module's manifest is `module.toml` at the top of its version folder.
- It MUST be TOML 1.0, UTF-8, readable by Rust's `toml` crate and Python's `tomllib`.
- It is a **superset of plugin.toml**: every plugin.toml key keeps its meaning.
- **Unknown keys are refused** at every level (`manifest.unknown-key`), as `Manifest::parse` does
  today. New keys come only with a new `hub` minor (§8.7); a module that uses them sets `hub` to
  that minor, so an older reader refuses it with `manifest.hub`.
- (v1.10 clarification, #1096) A reader checks `hub` **first**: a file whose `hub` is a
  `MAJOR.MINOR` it doesn't serve fails with `manifest.hub` before any other check, even when it
  has keys the reader doesn't know. For a newer `hub` (a higher minor, or a higher major) the
  reader may still take the file's **head**: `id` (§4.3), `name`, `version` (SemVer) and `hub`,
  each checked alone, nothing else read. The head is only for listing the module as
  `needs-hub` (§7.8), and its name is cut to 64 characters. Every reader (full, legacy and
  head) draws a module's `name` with C0/C1 controls, bidi marks and controls (U+061C,
  U+200E/F, U+202A–202E, U+2066–2069), zero-width and invisible characters (U+00AD, U+180E,
  U+200B–200D, U+2060–2064, U+FEFF), interlinear annotation marks (U+FFF9–FFFB) and tag
  characters (U+E0000–E007F) dropped, whitespace runs as one space, and the id when nothing is
  left.
- Strings compare exactly (case-sensitive) unless a rule says otherwise.

### 4.2 Keys

| Key | Type | Required | Meaning |
|---|---|---|---|
| `id` | string | yes | Identity and folder name (§4.3). |
| `name` | string | yes | Shown in the UI. 1–64 characters, a plain function name (§14). |
| `version` | string | yes | SemVer `MAJOR.MINOR.PATCH`, optional `-prerelease`, no `+build`. |
| `kind` | string | yes | §3. |
| `family` | string | yes | `hideout`, `foxglove`, `vulpes`, `digitalis` or `third-party` (§4.3). |
| `hub` | string | yes | Contract version it was written for, `MAJOR.MINOR` (VS Code's `engines`). v1 readers accept `1.0`; (v1.4) readers of hub 1.1 accept `1.0` and `1.1`. |
| `visibility` | string | no | `public` (default) or `private` (§13). |
| `description` | string | no | Plain words for the Modules page. |
| `api` | table, or string | no | Method-family API versions it calls, `{ foxglove = "1.13", vulpes = "1.0" }`. A plain string means `{ foxglove = "<string>" }`. |
| `requires` | array of strings | no | Modules needed at enable (§4.4). |
| `enriches` | array of strings | no | Third-party programs it works with, as `<name>.exe`. Missing ones give one hint; never block. |
| `run` | array of strings | `service`, `adopted` | Program and arguments. For `service`, relative and inside the version folder. For `adopted`, `run[0]` is absolute. Absent for other kinds (`manifest.run`). |
| `transport` | string | no | `pipe` (default) or `stdio` (legacy plugins, bridged by the hub). |
| `swap` | string | no | `overlap` (default) or `handover` (§7.6). |
| `provides` | array of strings | no | What it offers (§4.5). |
| `permissions` | array of strings | no | What it may touch (§6). |
| `[activation]` | table | no | When it starts (§4.6). |
| `[limits]` | table | no | (v1.10, hub 1.2) Its Job Object caps (§7.7): `memory_mb` (integer, 16 to 1,048,576: all its processes together, in MiB) and `cpu_percent` (integer, 1 to 100: a hard cap, in percent of the whole machine). Each absent value is the hub's maximum. |
| `[lifecycle]` | table | no | `data = keep | remove | ask` (default `ask`): what remove does with `data\`. |
| `[identity]` | table | no | Package identity (§4.8). |
| `[[ui.panels]]`, `[[ui.windows]]` | arrays of tables | no | UI contributions (§4.7). |
| `[[settings]]`, `[[actions]]`, `[[secrets]]` | arrays of tables | no | As plugin.toml, plus `when` (§4.7); (v1.4, hub 1.1) settings also take `kind = "file"` and `filter`. |
| `[[mcp.tools]]` | array of tables | no | Tools for the MCP bridge (§9). |
| `[[pin]]` | array of tables | `runtime` only | Upstream archives (§4.8). |
| `[edition]` | table | edition content only | §4.9. |

Checks every reader MUST make (error ids in §15):
- `id`, `family` and `kind` agree (§4.3): `manifest.id`, `manifest.family`, `manifest.kind`.
- `version` is SemVer as above: `manifest.version`.
- `hub` is `MAJOR.MINOR` and served (§8.7): `manifest.hub`.
- Every `api` value is `MAJOR.MINOR`: `manifest.api`. Whether a provider serves it is checked at
  enable (`lifecycle.api`).
- `run[0]` of a `service` stays inside its folder: only normal names or `.`; no `..`, root, drive
  or prefix (`manifest.run`). Today's rule.
- `visibility` is `public` or `private`: `manifest.visibility`.
- `[[secrets]]` needs `secrets` in `permissions`: `manifest.secrets`.
- Ids of `[[actions]]`, `[[ui.panels]]`, `[[ui.windows]]`, `[[mcp.tools]]` and names of
  `[[settings]]` are unique in their list and use `a-z`, `0-9`, `-`: `manifest.actions`,
  `manifest.ui`, `manifest.settings`, `manifest.mcp`.
- Every `when` and `activation.on` entry parses (§4.6): `manifest.activation`.
- (v1.10) `[limits]` has only `memory_mb` and `cpu_percent` (`manifest.unknown-key`), each an
  integer in its range (`manifest.limits`), and needs `hub = "1.2"` or later (`manifest.hub`). A
  value over the hub's maximum is valid: the supervisor clamps it (§7.7).
- (v1.4) A `kind = "file"` setting needs `hub = "1.1"` or later (`manifest.hub`); `filter` is for
  `file` settings only and follows §4.7 (`manifest.settings`).

Example, a public first-party service module (from the design's inventory, §2):

```toml
id = "foxglove.art"
name = "Art"
version = "0.4.0"
kind = "service"
family = "foxglove"
hub = "1.0"
api = { foxglove = "1.13" }
requires = ["foxglove.link>=0.4"]
run = ["foxglove-art.exe"]
swap = "overlap"
provides = ["method:art.*", "facts:art.*", "channel:previews"]
permissions = ["facts.read:keyboard.*", "channel.open:foxglove.link/art", "hub.notify"]

[activation]
on = ["fact:keyboard.connected", "open", "call"]

[[ui.panels]]
id = "art"
slot = "page"
title = "Art"
icon = "image"
when = "keyboard.connected"

[[actions]]
id = "next-art"
name = "Next art"
```

### 4.3 Ids and families

- An `id` is labels joined by `.`. Each label MUST match `[a-z0-9][a-z0-9_-]*`; the whole id is
  at most 128 characters.
- **One-label ids** (such as `vulpes`, `digitalis`, `obs`) are allowed for first-party modules
  only: family `hideout`, `foxglove`, `vulpes` or `digitalis`, listed as an exact id in that
  family's root scope (§12.3). Every other id has two or more labels (`manifest.id`).
- **Reserved first labels:** `hideout`, `foxglove`, `vulpes`, `digitalis`, and `arete` (alias of
  `vulpes`). A module whose id starts with one MUST have the matching `family`.
- A `third-party` module MUST NOT use a reserved first label, and its id MUST be reverse-DNS
  (two or more labels).
- The legacy prefix `org.foxglove.` (today's plugins) is family `foxglove`.
- Readers MUST accept `arete` as an alias of `vulpes` in ids, `family` and `api` keys, and MUST
  write `vulpes`. `C:\AI` stays Vulpes's home.
- A `private` module's id MUST NOT start with a reserved first label (§13.2).

### 4.4 `requires`

- Each entry is `<id>` or `<id>>=<version>`. `<version>` may be shortened to `MAJOR` or
  `MAJOR.MINOR`. No other operators (`manifest.requires`).
- Checked at enable (`lifecycle.requires`). Cycles are refused.
- At runtime a required module that is gone is only "provider gone": calls get error 2 and the
  dependent shows "Waiting for <module>" (§7.8). The hub never restarts a module because another
  needs it.
- Disabling a module that an enabled module requires MUST be refused with
  `lifecycle.required-by`, naming the dependents, unless they are disabled too.
- The dependency graph for root-cause states (§7.8) is `requires` plus `channel.open:`
  permissions.
- **Who waits (v1.10).** A provider is *down* while its state is `off`, `restarting`,
  `crash-stopped`, `install-failed`, `revoked`, `needs-hub` or `quarantined`; not while it is
  `starting`, `updating`, waiting for a call (no state) or itself `waiting`. A module that is
  `running` with a down module anywhere along what it needs reads `waiting`; one in any other
  state keeps that state. The root it names is the first down module on the way, or, if that
  one needs a down module itself, the deepest such one (Lights opens a channel on Art, Art
  requires Link, Link crashed: both Lights and Art name Link).
- A `public` module MUST NOT require a `private` one (§13.2).

### 4.5 `provides`

Each entry is `<what>:<argument>`. Unknown `<what>` is refused (`manifest.provides`).

| Entry | Means | Kinds |
|---|---|---|
| `method:<pattern>` | Serves these methods through the router. A method name or a prefix ending in `.*`. Fixed by the catalog (§6.3). | service, adopted |
| `facts:<pattern>` | Publishes these facts. | service, adopted |
| `channel:<name>` | Offers a direct channel `<id>/<name>` (§8.4). Its capability line comes from the catalog (§6.2). | service, adopted |
| `content:<type>` | Holds content: `board-profile`, `art-pack`, `lighting-preset`, `lighting-catalog`, `wallpaper-manifest`, `agent-pack`, `style-pack`, `eval-pack`, `template-pack`, `edition`. | content |
| `firmware:<board>` | Holds firmware images for a board. | firmware |

- A `content` module MUST provide at least one `content:` entry; a `firmware` module at least one
  `firmware:` entry.
- plugin.toml's `Provides` values (`facts`, `panel-layout`, `lighting-effect`, `action`,
  `backend`) are still read in legacy manifests (§4.10).

### 4.6 Activation events and `when` clauses

`[activation] on` lists when the hub starts the module. Default `["enable"]`.

| Event | Starts the module when |
|---|---|
| `enable` | it is enabled (and at hub start, if enabled) |
| `open` | one of its panels is shown |
| `call` | a method it provides is called |
| `fact:<pattern>` | a fact matching the pattern is set |
| `app:<name>.exe` | that app's window appears (reported by `hideout.appwatch`) |
| `device:<class>` | short for `fact:devices.present.<class>` (UX12) |

Rules:
- A module with a key bound to one of its actions, or a pinned quick action, MUST be started at
  enable, whatever `on` says (UX3).
- Activation is by events only, never timers.
- A module started by an event stops by its own decision (exit code 0); the hub does not stop
  idle modules.
- **How each event fires (v1.10).** An event starts the module only while it is enabled, not
  running or starting, and not held by safe mode (§7.10); an event for any other module does
  nothing, so a second event while it starts never starts a second instance.
  - `fact:<pattern>`: a fact matching the pattern (§8.3 patterns) changes to a value that is
    true by the `when` rule (set and not `false`, `null`, `0` or `""`). Setting it to the same
    value again is no change. A matching fact that is already true when the module is armed
    (hub start, once the supervisor knows its modules; Turn on; Start normally) starts it then.
  - `app:<name>.exe`: the app watch's `apps.running.<name>` turns true (same `<name>` rule as
    `apps.seen`). A hub without the app watch never fires it.
  - `device:<class>`: `devices.present.<class>` turns true. The hub publishes no device facts
    itself: it fires once the first-party device module publishes them.
  - `open`: a `page` panel of the module comes on screen (the active tab of a shown hub window,
    or a shown pop-out), whether or not the module registered it yet. A tile in the tray panel
    is not `open`.
  - Once it has stopped by itself (exit code 0), the next event starts it again.

`when` (on `[[ui.panels]]`, `[[ui.windows]]`, `[[actions]]`, `[[settings]]`) decides whether the
item shows:
- Grammar v1: one fact name, optionally prefixed by `!`. No operators.
- True when the fact is set and its value is not `false`, `null`, `0` or `""`.
- The hub evaluates it from its facts cache (§2.4 includes cached facts), so a cold module's
  items still show or hide correctly.
- A module whose devices aren't present (all its `page` panels false) shows no sidebar page, only
  a row on the Modules page (UX12).
- `devices.present.<class>` facts are published by the first-party device module from
  catalog data, never by the hub.
- **`apps.seen.<name>` (v1.4, optional fact).** `true` once the app `<name>.exe` has had a window
  on this PC, kept across restarts; published by `hideout.appwatch`, never by the hub. `<name>` is
  the exe name without `.exe`, lowercased, as one label (`[a-z0-9_-]`; other characters become
  `-`): `apps.seen.obs64`. A `when` naming `apps.seen.` with more than one label after it is
  `manifest.activation`. The fact is **optional**: a hub without `hideout.appwatch` never sets it,
  so `when = "apps.seen.<name>"` is false there and the item stays hidden until the user shows it
  (an `enriches` app's tile is a typical use). A module MUST NOT depend on it for anything but
  showing or hiding. (v1.7) `hideout.appwatch` sets it once a process `<name>.exe` has run.
- **`apps.running.<name>` (v1.7, optional fact).** `true` while a process `<name>.exe` runs,
  `false` while none does; same `<name>` rule as `apps.seen`. A module uses it to tell "the app
  refused" from "the app isn't open": not set means no app watch, which is neither.
- **The app watch (v1.7).** `hideout.appwatch` is part of the hub. It watches only the apps some
  installed manifest asks about: an app fact in a `when`, an `enriches` entry, an
  `app:<name>.exe` activation event or an `apps.watch:<name>.exe` permission. App names come
  only from manifests. It is event-driven (a process snapshot at start, the watched processes'
  exits, and new top-level windows), never a timer; an app counts as started once it creates a
  window, a hidden one included. With nothing to watch it does not run.
- (v1.7) `apps.*` facts are the hub's: a module can't register them (`permission.provider`) or
  `facts.set` them (error 5). A module holding `apps.watch:<name>.exe` may `facts.get` and
  `facts.subscribe` `apps.running.<name>` and `apps.seen.<name>` (exact keys); `facts.read`
  covering them also works.

### 4.7 Contributions (data, VS Code-style)

Contributions are declared in the manifest so the hub can show them before the module runs.

**`[[ui.panels]]`:** `id`, `slot`, `title`, `icon`, `when?`.
- `slot` is `page`, `tile`, `status`, `settings` or `action` (§5.1).
- The tree comes later in `ui.register` (§5.1) or from the cache. A `ui.register` panel id not
  declared here is rejected (`ui.undeclared`).

**`[[ui.windows]]`:** `id`, `title`, `when?`. A module window (§5.7). The hub shows an Open
button and sends `ui.window.open`. Needs the `ui.window` permission: a manifest with a
`[[ui.windows]]` entry and no `ui.window` in its permissions is refused (`manifest.window-missing`,
v1.1).

**`[[settings]]`:** as plugin.toml, plus `when?`.
- The hub draws the form, generated when the module gives no `settings` tree.
- Values live in the module's settings folder (§2.3). The hub reads and writes them only through
  `settings.get` and `settings.set` (§8.8).
- **`kind = "file"` (v1.4, hub 1.1):** a path the user picks. The form shows the current file name
  and a **Choose file** button that opens the native file picker; the value is the full path as a
  string, `""` for none (the `default` is a string). An optional `filter` lists the file names the
  picker offers: patterns with `*` and `?`, joined by `;`, 1 to 128 characters, no `/`, `\`, `:`,
  quotes, `<`, `>` or `|` (`filter = "ExampleSDK*.dll"`). The hub sends the chosen path with
  `settings.set`, so it takes effect at once; the module checks the file and refuses with
  `data.reason` as for any setting. A legacy `plugin.toml` keeps its three kinds (`bool`, `int`,
  `text`); to a plugin.toml reader a `file` setting is `text`.

**`[[actions]]`:** as plugin.toml, plus `when?`.
- The user binds keys in the hub. The hub sends `action.invoke`.
- The module's page shows each bound key next to its action (UX3).
- **Keys section (v1.4):** the hub draws it, not the module. Every `page` panel of a module with
  `[[actions]]` (those whose `when` is true) gets a generated **Keys** section after the module's
  tree, like the settings form: one row per action with its `name` and the bound key, or
  **Set key**. A module MUST NOT draw its own key list.
- **Bound keys (#991, hub behaviour, no wire change):** a key is F13 to F24 (modifiers
  allowed), or any other key with two of Ctrl, Alt and Shift; a chord that types a character
  on the current keyboard layout (AltGr) and the chords Windows keeps are refused. The hub keeps them in `hideout.toml`'s `[keys]` table
  (`"<module>/<action>" = "Ctrl+Alt+M"`), registers them system-wide, and a press sends
  `action.invoke {action, source: "key"}` (§8.8), with the window in the tray too.

**`[[secrets]]`:** as plugin.toml.

**`[[mcp.tools]]`:** §9.

### 4.8 `[[pin]]` and `[identity]`

`[[pin]]` (runtime modules; Arete's component pins, now signed through the catalog):

| Key | Type | Required | Meaning |
|---|---|---|---|
| `url` | string | yes | `https://` URL of an immutable upstream file |
| `sha256` | string | yes | 64 lowercase hex characters |
| `size` | integer | no | Size in bytes; the download limit |
| `unpack` | string | no | `zip` (default) or `none` |
| `strip_top` | bool | no | Drop the archive's top folder. Default `true` |

`[identity]` (picks D, E): `application = "<name>"`. The module's exe gets that Application in
the one Hideout sparse package. Only modules that need package identity (today the hub and
`hideout.lights`) set it. Allowed only for `public` first-party modules.

### 4.9 Editions

An edition (Lagopus, Zerda, Velox) is a `content` module with `provides = ["content:edition"]` and
an `[edition]` table:

```toml
[edition]
family = "vulpes"
modules = ["<id>", "<id>"]
```

- `modules` lists the family's modules the edition has **enabled**. Every other module of that
  family is disabled when the edition is chosen.
- Choosing an edition MUST only enable and disable. It MUST NOT install, update or remove. A
  listed module that isn't installed is reported (`lifecycle.edition`); the rest still applies.

### 4.10 Legacy manifests: `plugin.toml` and `kit.toml`

They keep working. A reader looks for `module.toml`, then `plugin.toml`, then `kit.toml`. A legacy
file:
- is parsed exactly as `Manifest::parse` does today;
- MUST NOT contain keys new in this spec (they are unknown keys there);
- reads as: `kind = "service"`, `transport = "stdio"`, `swap = "overlap"`, `hub = "1.0"`,
  `visibility = "public"`, `api = { foxglove = "<its api>" }`, `[activation] on = ["enable"]`,
  `[lifecycle] data = "remove"`, and `family = "foxglove"` for `org.foxglove.*` and
  `foxglove.*` ids, otherwise `third-party`.
- **The legacy root's scope (v1.1):** a legacy `foxglove-plugin-root` (§12.2) reads as one family,
  `foxglove`, whose scope is exactly the ids `foxglove.*` and `org.foxglove.*`, nothing else.
- **Panels for a legacy plugin (v1.4).** A plugin.toml can't declare `[[ui.panels]]` (no new keys).
  A plugin that wants a tile or a page ships a `module.toml` next to its plugin.toml, the
  superset: the same `id`, `name`, `version`, `run`, `permissions`, `[[settings]]`, `[[actions]]`
  and `[[secrets]]`, plus `kind = "service"`, `family`, `hub`, `transport = "stdio"`, `api`
  (its plugin.toml `api`) and its `[[ui.panels]]`. A reader takes the `module.toml` (precedence
  above); Foxglove, which reads only plugin.toml, keeps working. The plugin's program does not
  change transport: it still speaks the v1 wire on stdin and stdout, and the hub's stdio bridge
  carries its `ui.*` messages (§8.2).
- **Which API a legacy `api` names (v1.6).** A plugin.toml's `api` is the Foxglove plugin API
  version (`crates/plugin-api`'s `API_VERSION`, the one `plugin-host` serves), and a reader
  checks it against that version (`lifecycle.api` when not served), never against `hub`.

## 5. Panels across the process boundary

Pick B. Design §4.3.

### 5.1 Registration and slots

A module registers its panels after `initialize`:

```
ui.register {instance, panels: [{id, slot, title, icon, tree}]}
```

| Slot | Where it shows |
|---|---|
| `page` | a sidebar page |
| `tile` | the tray panel |
| `status` | the status strip |
| `settings` | the module's settings (generated from `[[settings]]` if no tree) |
| `action` | a tray menu item |

- **Tiles (v1.4).** A `tile` is a small fixed card in the tray panel. The hub draws its frame
  (icon, title, the ··· menu, the module's state, the stale view) from the manifest and
  `module.<id>.state`; the module's tree is only the body, under the tile caps (§5.5). A click
  outside the body's button opens the module's first `page` panel.
- **Icons (v1.4).** `icon` names one of the published icons, listed with the schema
  (`schema/hideout-v1.json`, key `icons`; `ICONS` in the Rust crate and the Python models):
  `module`, `battery`, `bell`, `bluetooth`, `book`, `brush`, `calendar`, `camera`, `chart`,
  `chat`, `cloud`, `folder`, `gamepad`, `gear`, `headphones`, `image`, `info`, `key`,
  `keyboard`, `lights`, `link`, `lock`, `mic`, `music`, `palette`, `play`, `screen`, `sparkle`,
  `speaker`, `terminal`, `timer`, `video`, `wifi`. Any other name is not an error: it draws the
  generic `module` icon, so a newer name degrades on an older hub. The list grows only with a
  spec minor; the hub draws each icon from its theme, never from module files.

### 5.2 The tree

Nodes are `{type, id, props, bind?, stream?, children}`.

- **Layout:** `column`, `row`, `grid`, `section`, `tabs`.
- **Widgets:** `label`, `value`, `button`, `toggle`, `slider`, `choice`, `color`, `text_input`
  (multi-line, optional attachments), `list`, `table`, `meter`, `image`, `gallery`, `log`,
  `markdown` (safe subset, code blocks, no HTML), `text`.
- **`canvas`:** a retained set of shapes (rect, rounded key, circle, text) with ids, (v1.8) at most
  1,000 in one canvas (§5.5; each shape is a node too). A click returns the shape id.
- **Box sizes (v1.5).** A `canvas` declares its unit size, `width` and `height` (the units its
  shapes use; numbers above 0). The hub scales the canvas uniformly to the node's width, so the
  box's aspect is `width / height`; without them it is the shapes' bounding box. An `image`
  declares `aspect` (width / height, a number above 0) and optionally `width` (its natural width
  in layout pixels, above 0). Without `aspect`, an image with an asset takes the asset's aspect
  and a streamed image takes the first frame's, once; later frames of another size are fitted
  inside the same box (letterboxed), never stretched or cropped, so the page never moves.
- **`scaling` (v1.5)** on `image`: `"smooth"` (filtered, the default, for photos and art) or
  `"pixel"` (whole-number steps, no blur, for pixel art and 1-bit screens). An unknown word draws
  as `smooth`; a `scaling` that isn't a string is `ui.prop`.
- **`mono` (v1.5)** on `text`: `true` draws it in the theme's monospace face (logs, code); `false`,
  the default, in the body face. Not a boolean: `ui.prop`.
- **`bind`:** points a prop at a fact (`fact:keyboard.layer`). The hub fills it from its cache and
  redraws only on change.
- `id` MUST be unique in the panel and stable across registrations (view state is keyed by it).
- The vocabulary is versioned with `hub`. An unknown node type draws as "needs a newer Hideout".
- **`tone` (v1.4)** on `value` and `label`: what the text means, so the hub can colour it without
  knowing the domain. One of `neutral` (text colour, the default), `active` (following,
  counting), `live` (muted, recording, on air), `ok` (resting, done), `attention` (needs something
  from the user) and `off` (idle, outside app closed). The hub maps each word to a theme token at
  4.5:1 or better on its surface. An unknown word draws as `neutral`; a `tone` that isn't a string
  is `ui.prop`. Colour never stands alone: the text says the state.
- **`opens` (v1.4)** on `button`: the hub navigates without a round trip to the module.
  `"page"` opens the module's first `page` panel (as a tab, §5.9), `"page#<node id>"` opens it
  scrolled to that node (a `section` is expanded; a node that isn't there opens the page at the
  top), `"settings"` opens the module's settings. Only the module's own panels; any other value is
  `ui.prop`. No `ui.event` is sent.
- **`action` (v1.4)** on `button`: the button is that `[[actions]]` entry. A click sends
  `action.invoke {action, source: "panel"}` (§8.8), the same call a bound key sends, and is
  queued like a key press while the module restarts (UX3). The hub shows the bound key as a chip
  next to the button. An id that isn't action-shaped (`a-z`, `0-9`, `-`) is `ui.prop`; one that
  names no `[[actions]]` entry of the module is `ui.action`. No `ui.event` is sent.
- A button has at most one of `opens` and `action` (both: `ui.prop`); with neither it sends
  `ui.event` as before. `tone`, `opens` and `action` are optional props: a hub of contract 1.3 or
  older ignores them (the button then sends `ui.event`), so a module that must run there also
  answers that `ui.event`.

### 5.3 Updates, input and assets

- `ui.patch {panel, ops}` changes nodes in place. At most ~20 per second per panel; the hub
  coalesces beyond that. Patches carry state; anything that moves uses a stream.
- (v1.5, written down) A `set` op merges its `props` into the node: each named prop is replaced,
  and a prop set to `null` is removed; props it doesn't name stay. On a streamed `text` or
  `markdown` node, setting `text` replaces everything shown, the appended text included; the
  stream keeps appending after it (§5.4).
- `ui.event {panel, node, kind, value}` goes to the owning module only, checked like any call.
  An error result means "refused": the hub reverts optimistic input and shows `data.reason`.
- `ui.asset.put {hash, mime, data}` uploads an image once, keyed by SHA-256, at most 256 KiB.
  Nodes refer to it by hash. (v1.7) The hub's panel host answers it: the router passes the call
  on and sends its answer back, `{}` once stored, or error -32602 with `data.id` `ui.limits`
  and `data.reason` (not an image, not 64 hex digits, over 256 KiB, bad base64). Params of the
  wrong shape are -32602 from the router; no answer within 30 s is error 4. The whole message
  is still at most 256 KiB (§8.1), so an asset sent over the pipe is at most about 190 KiB.

### 5.4 Live streams

Some nodes take a `stream` prop naming one of the module's channels (§8.4):

| Node | Stream carries |
|---|---|
| `canvas` | colours for shapes, by shape id |
| `image` | small frames |
| `text` | appended text (chat tokens) |
| `markdown` (v1.5) | appended markdown source (a reply as it is written); the hub redraws it |

- The hub opens the stream as a normal channel with the same grant. Frames never pass through the
  router.
- The stream runs only while the panel is visible and the window isn't minimised, at most 30
  frames a second. The hub closes the channel otherwise.
- (v1.3) The hub also tells the module, with `ui.visibility` (§5.9), so work that feeds no stream
  (polling a device, rendering previews) can stop too.
- The hub knows nothing of the domain: it colours shapes or appends text the module declared.
- Rule: a page that shows what your devices (or your AI) are doing stays in the hub.
- **Where (v1.5).** Streams are for `page` panels (and their pop-outs). A `tile` has none: its
  stream cap is 0 (§5.5); a tile that changes uses patches (a countdown at 1 a second is well
  under the patch rate).
- **"No new frames" (v1.5).** A channel has no keep-alive, so a still scene and a stalled stream
  look the same. A `canvas` or `image` stream node may say how long a gap is normal with
  `stale_after_ms` (a whole number of milliseconds above 0): when no frame arrives for longer,
  the hub dims the last frame and says when it was seen. Without it the hub never does. On `text`
  and `markdown` it is accepted and ignored (silence is normal for text). Not a whole number above
  0: `ui.prop`.
- **Text streams (v1.5).** Frames append (§8.4 kind 3); replacing or clearing is a patch of the
  node's `text` (§5.3), and the stream keeps appending after it. Appended text has its own cap
  (§5.5), past which the hub drops the oldest whole lines.
- **Hidden and back (v1.5).** The hub keeps a text node's text while the panel is hidden; what the
  module appended meanwhile never arrived. The hub sends `ui.visibility visible` (§5.9) before it
  opens the panel's streams again, and applies a patch the module sent before its answer to a
  stream's `channel.create` before that stream's first frame. So a module that appends while
  hidden SHOULD, on `visible`, patch the node's full text before it answers `channel.create`, then
  stream appends again. Text over the 16 KiB node cap: patch `text` to `""` and send it all as
  the first frames of the new stream (up to the stream text cap).
- **When the provider closes (v1.5).** A stream the provider closes while the panel stays visible
  and the module is `running` is reopened (a new `channel.create`) after 1, 2 and 4 s
  (`STREAM_REOPEN_S`). When the third reopen ends too, the node shows "Stopped updating" with
  Try again, which reopens once. A module restart, or the panel turning `visible` again, starts
  the count afresh. A stream refused or closed for `ui.limits`, or one whose grant fails, is not
  reopened. A patch that removes a node's stream (`stream` set to `null`, or the node removed or
  replaced) closes its channel cleanly: no state is shown and nothing is reopened; a patch that
  names another channel closes the old one and opens the new one.
- **Visibility is per panel (v1.5).** `visible` (§5.9) is the panel's. A stream node scrolled out
  of view, or a window covered by another, still streams; the hub may skip drawing it. A
  node-level rule may come in a later minor.
- **Screen reader (v1.5).** A streamed `image` MUST have a `label` (a non-empty string): its
  accessible name, which frames never change. Without one it is `ui.prop`. A canvas is named by
  its `label` too when it has one; its shapes by their text.
- **Reduced motion (informative, hub-only).** With the system's animation effects off the hub
  draws canvas and image streams at most 2 frames a second and shows a small "Live" dot on a live
  one (at that rate motion no longer says live); without reduced motion there is no mark. The
  module isn't told and sends frames as usual; the hub may read the channel less often. UX:
  [hideout-streams-ux.md](hideout-streams-ux.md).

### 5.5 Validation and the panel guard (UX5)

- `ui.register` and `ui.patch` MUST be validated: node count, depth, string length, canvas shape
  count and table rows each have a cap, published with the schema. (v1.4) The `tone`, `opens` and
  `action` props (§5.2) are checked the same way, on the tree after a patch too; so are (v1.5) the
  box sizes, `scaling`, `mono`, `stale_after_ms` and a streamed image's `label` (§5.2, §5.4). Over a cap: that panel is
  rejected (`ui.limits`), the others register.
- **Caps (v1.1):** per panel, at most 2,000 nodes per tree, depth 32, 16 KiB (16,384 UTF-8 bytes)
  of text per node, 8 live streams, 4 MiB per stream frame and 60 frames a second per stream. A
  value at the cap passes; one over it is `ui.limits`. A stream over its count, frame size or rate
  is refused or closed with the same error (the hub still draws no more than 30 frames a second,
  §5.4).
- **Canvas and table caps (v1.8):** at most 1,000 shapes in one `canvas` (its children) and 10,000
  rows in one `table` (its `rows`), per node. Shapes count as nodes too, so the 2,000-node cap
  still holds for the panel. At the cap passes; one over is `ui.limits` for that panel.
- **Published (v1.8):** the page caps are the schema's `limits` (`nodes`, `depth`, `text_bytes`,
  `canvas_shapes`, `table_rows`, `streams`, `frame_bytes`, `frames_per_s`, `message_nesting`) and
  the tile caps its `tile_limits`. A reader takes the numbers from there; this section and the
  schema agree.
- **Refused or closed (v1.8):** the stream count is a tree cap: a 9th stream node makes that panel
  `ui.limits` at `ui.register` (the other panels register), or refuses that `ui.patch`, and no
  channel opens for it. Frame size and rate are checked on the channel: a frame over 4 MiB, or a
  61st frame inside one second, makes the hub close that channel. It logs `ui.limits` with the
  reason, the node shows "This preview couldn't be shown." with Open log, and the stream is not
  reopened (§5.4), not even when the panel turns `visible` again, until the module's next
  instance registers (a restart or a swap). The module is not told by message; its end of the
  channel closes. Nothing is counted as a crash.
- **Stream text cap (v1.5):** text a stream appends to one `text` or `markdown` node is capped
  separately at 1 MiB (1,048,576 UTF-8 bytes; `streams.text_bytes` in the schema). Past it the
  hub drops the oldest whole lines (up to and including a line feed) until the rest fits, and
  shows that earlier text was trimmed; a single line over the cap keeps its newest bytes, cut at a
  character boundary. Trimming is not an error. Text in the tree or set by a patch keeps the
  16 KiB per-node cap.
- **Tile caps (v1.4):** a `tile` panel's tree is the tile body only, at most 12 nodes, depth 3,
  64 UTF-8 bytes of text per string, one `button` and (v1.5) no stream; its other caps are the
  page caps. Over any of
  them that tile is `ui.limits` and the module's other panels register. The frame is hub-drawn and
  not counted. A tile body is meant to be one `value` (the glance, with a `tone`), at most one
  `label`, at most one `meter` and at most one `button`.
- **Message nesting (v1.1.1):** one wire message nests at most 128 brackets deep (arrays and
  objects together, strings ignored). A reader counts this on the raw text without recursion and
  refuses a deeper message whole with `ui.limits` before any recursive parser runs, so a hostile
  frame cannot overflow the stack. A panel tree at the depth cap (32) needs about 70 of the 128.
- Each panel draws in its own guarded scope. A panic greys that panel ("<Module>'s page couldn't
  be shown"), not the hub.
- The hub keeps a breadcrumb of the panel being drawn. Two hub restarts within a minute with the
  same breadcrumb start the hub with that module's panels **quarantined**, offering Start again
  or Go back to <previous version>.
- (v1.10) A module whose panels are quarantined is stopped and not started by anything (hub
  start, activation, a restart) until **Start again** (panels released, the module started
  with a fresh crash count) or **Go back** (the rollback, then the start). Its stop is not a
  crash; its state is `quarantined` (§7.8).

### 5.6 Live rebuild

- A new registration (a new instance after a swap or restart) replaces that module's panels in one
  step.
- The hub keeps each panel's view state by node id: scroll position, open sections, selected tab,
  text being typed.
- A module that goes away greys its panels with its state (§7.8). It does not remove them.

### 5.7 Pop-outs and module windows

- **Pop-out:** any page can pop out into a hub viewport (same panel, same AUMID). It needs no
  module code, so it works for every language.
- **Module window:** only where free-form editing can't be expressed as panels: the keymap editor,
  the LED wizard, the calibrator, and the overlay (a topmost click-through layer, not a page).
  These are the module's own windows and follow §2.5 (v1.6: the module keeps their placement;
  Keep on top is for pop-outs).

### 5.8 Vulpes in the hub (pick C)

- The Vulpes module (`arete hub` today) is a Python process on the hub's pipe. It calls Arete's
  core in process (broker, agents, chat, image, vault).
- It MUST NOT start the web server unless the user turns on "Use from phone or tailnet". The
  browser UI is for the phone and the tailnet only.
- Its full UI is native panels, and each page can pop out: AI (tile and status), Chat, Images,
  Agents, Models, Schedules, Settings (design §4.3.1 has the node list per page).
- Chat tokens stream through a `text` stream, never patches.
- Chat draws from the panel cache first (§2.4), never waiting on Python's start-up.
- Exception until a `paint` node exists: "Change part" opens in the browser, marked as such.
- Safety stays in Arete's core: the GPU lock, the under-18 rule and the privacy filter apply to
  every UI path.

### 5.9 Tabs (v1.3)

Open `page` panels show as tabs above the page area, like editor tabs in VS Code. The sidebar
stays the launcher; the tabs are what is open. UX, states and wording: [hideout-tabs-ux.md](hideout-tabs-ux.md).

- **Title and icon:** a tab shows the panel's `title` and `icon` from `ui.register` (§5.1). No
  new field. Before the module answers, the hub draws them from the cache (§2.4).
- **Badge:** `ui.badge {panel, badge, count?}` (notification, M → H) sets the tab's badge:
  - `none`: no badge (the default);
  - `attention`: a dot, "something new wants a look";
  - `dirty`: unsaved work. Closing the tab, Close page, or Quit Hideout asks first (§2.5).
    (v1.6; was: closing its pop-out too) Closing a pop-out docks the page back and never asks;
  - `count` with `count` ≥ 1: a number; the hub shows `99+` above 99.
  A new `ui.register` from a new instance resets the badge to `none`. A badge for a panel the
  module didn't register, or for a slot other than `page`, is ignored and logged. Badges follow the
  patch rate (§5.3). The hub may also show the badge on the panel's sidebar entry.
- **Visibility:** `ui.visibility {panel, state}` (notification, H → M), sent after `ui.register`
  and then only on change:
  - `visible`: on screen (the active tab, or a pop-out that isn't minimised). (v1.6) A pop-out
    on screen stays `visible` while the hub's main window is in the tray;
  - `hidden`: still open but not on screen (a background tab, a minimised window, (v1.6) a tab
    or placeholder of the hub's main window while that window is in the tray; was: "the hub
    hidden in the tray"). The hub has closed the panel's streams (§5.4) and keeps what they drew (v1.5:
    the last frame, a text node's text). On the way back the hub sends `visible` before it opens
    the streams again;
  - `closed`: the user closed the page: its tab, or (v1.6) Close page in its pop-out. Closing a
    pop-out (×) docks the page back and is never `closed` (§2.5). The module SHOULD stop work for
    that panel. It keeps running; the panel stays registered and opens again from the sidebar.
  (v1.6) Through a pop-out: popping out sends nothing new if the page was the active tab (it
  stays `visible`), else `visible`; minimising it `hidden`; docking back `visible` if it lands as
  the active tab of a shown main window, else `hidden`.
  A module needs no answer and MAY ignore both messages.
- **Tiles (v1.4).** `ui.visibility` is sent for `tile` panels too: `visible` while the tray panel
  is open (and the tile isn't hidden by the user), `hidden` otherwise. A tile is never `closed`.
  A module that patches a tile on a timer (a countdown) SHOULD stop while its tile is `hidden`.
  `ui.badge` stays page-only.
- **Hub-owned, not in the contract:** tab order, the active tab and which tabs to restore after
  a restart live in the hub's own files (§2.3). (v1.6) Pop-out placement is hub-owned too, but its
  file shape is written down (`[windows]`, §2.5) so the hub's readers agree; no module reads it. A module can't open, close
  or reorder tabs; it can ask for attention with a badge or `hub.notify`.
- **States:** a tab shows its module's state (§7.8) from `module.<id>.state`: a spinner for
  `starting`, `updating` and `restarting`, a warning mark for `crash-stopped`, `install-failed`,
  `needs-hub` and `quarantined`, greyed for `off`. The panel's header has the reason and the ways
  out. A removed module's tabs close.

## 6. Capabilities and permissions

### 6.1 Grammar

- One namespace; the hub checks every call. No provider prefix. (The pre-hub draft's
  `foxglove:`/`vulpes:` prefixes are dropped; readers MAY strip a known prefix from a legacy
  string.)
- **Kept from Foxglove API v1** (`crates/plugin-api/src/permission.rs`): `facts.read:<pattern>`,
  `facts.write:<pattern>` (never `device.*` or `*`), `devices.read`, `lighting.write`,
  `lighting.lease`, `behaviors.invoke`, `screens.write`, `art.write`, `key.events`, `hid.raw`,
  `profiles.write`, `focus.write`, `net`, `net.connect:<host>:<port|*>`,
  `net.listen:udp|tcp:127.0.0.1:<port>`, `files`, `audio.inputs.read`, `audio.inputs.mute`,
  `secrets`, `app.windows`, `apps.watch:<name>.exe`.
- **New:**

| Permission | Lets the holder |
|---|---|
| `provide:<method pattern>` | register as provider of those methods (only if the catalog allows it, §6.3) |
| `channel.open:<module>/<channel>` | open that channel |
| `channel.offer:<channel>` | offer that channel |
| `device.hid:<usage or catalog class>` | open that class of HID devices |
| `gpu.status` | read the GPU broker's state |
| `gpu.request:<class>` | be a GPU tenant of `chat`, `image` or `any` |
| `ai.ask:<agent>` | ask a Vulpes agent (`*` for any) |
| `image.generate` | start an image job |
| `secrets.ref:<scheme>` | pass secret references for Vulpes's vault; never values |
| `hub.notify` | send a notification through the hub |
| `ui.window` | open its own top-level window |
| `modules.manage` | install, enable, disable, reload modules (§6.6) |

- Unknown body: `permission.unknown`. Bad argument: `permission.argument`.
- **Sensitive** (red in the dialog): `key.events`, `hid.raw`, `net`, `ai.ask:*`, `secrets.ref:*`,
  and any channel of risk class `keys` (§6.2).

### 6.2 Channels say what they give (UX7)

- Each offered channel has a catalog entry with a **plain-language capability line** and a
  **risk class**: `normal`, `sensitive` or `keys`.
- The permission dialog MUST show the capability line, never the channel name alone.
- A channel that carries key presses (the link's events feed, and any bridge that forwards it,
  such as the Wallpaper Engine feed) is class `keys`: the same class as `key.events`.
- A channel with no catalog entry (a sideloaded or private provider without one) is shown as
  `sensitive` with "Unknown data from <module>".

### 6.3 Providers are fixed by the catalog

- The catalog entry of a module lists the method patterns it may `provide`. The hub refuses any
  other registration: `module.register` lists it under `rejected` with `permission.provider`.
- A third-party or private module MUST NOT provide a method area a first-party catalog assigns
  (`lighting.*`, `screens.*`, `art.*`, `gpu.*` …), so it can't capture other modules' calls.

### 6.4 Approval

- First-party modules shipped in the MSI are approved at install.
- Catalog, private and sideloaded modules show the permission dialog at enable.
- An update that adds permissions keeps the old version running and shows "Update waiting for
  you" (§7.8). Never a modal at sign-in.
- Outside tools ask in `initialize` and are approved once per tool.
- The cut-over from Foxglove carries every existing grant over silently. Only new permissions ask.
- Grants are machine state: they are stored in `inventory.json` (`granted`), in canonical form,
  sorted, never in `hideout.toml`.

### 6.5 Signing is provenance, not permission

A catalog signature (any trust anchor, §12, §13) proves where files came from. It never grants,
widens or skips a permission prompt.

### 6.6 The never-list

Never grantable to a third-party module, a private module from an unsigned source, an outside
tool or MCP:
- behaviours marked `Confirm` (bootloader, reset, clearing Bluetooth);
- another module's secrets;
- starting or ending a gaming reservation (only the user's own click, §8.9);
- `key.events`, `hid.raw` and `keys`-class channels **over MCP**, always;
- `modules.manage`, except to `hideout.modules` and the first-party `hideout` CLI.

`facts.write:*` and `facts.write:device.*` (§6.1) are never-list items: asked for in a manifest or
`initialize` they are left out of `granted` and reported as `permission.never` (v1.1), not
`permission.argument`.

A third-party or private module MAY hold `key.events`, `hid.raw` or a `keys` channel only when
sideloaded or signed by a user key, with the red warning. A manifest or `initialize` asking for a
never-list item gets it left out of `granted` (`permission.never`); calls needing it answer
error 5.

## 7. Lifecycle

### 7.1 Who does what

- **`hideout-core`** (Rust crate) implements install, verify, update, rollback and remove, and is
  the only writer of `inventory.json` (under `inventory.lock`).
- **`hideout.modules`** (a module) and the **`hideout` CLI** are its front ends.
- **The hub** reads `inventory.json` (it notices changes by a file-change notification, never by
  polling), owns `hideout.toml`, enables and disables, starts, swaps and stops.
- The CLI's `enable`, `disable` and `reload` go through the hub (`modules.*`, §8.8). File steps
  work without the hub running.
- Status never starts anything: `list`, `verify` and every status call are read-only.

### 7.2 Versioned folders

- Each version installs into `modules\<id>\<version>\`. The running version is never overwritten.
- The previous version folder is kept until the new one has run cleanly for a day (for automatic
  rollback, §7.8); then it is removed. At most two versions per module.
- Dev builds go to a shadow folder (Windows locks a running exe), and only in lab hubs (§7.6).

### 7.3 `inventory.json` and `hideout.toml`

`inventory.json` (what is installed; UTF-8 JSON, written atomically):

```json
{
  "format": "hideout-inventory-v1",
  "modules": {
    "<id>": {
      "version": "0.4.0",
      "kind": "service",
      "family": "foxglove",
      "visibility": "public",
      "source": "<source id>",
      "anchor": "<trust anchor id>",
      "channel": "stable",
      "installed": "2026-11-09T12:00:00Z",
      "files": { "<path>": "<sha256>" },
      "footprint": [],
      "previous": "0.3.2",
      "granted": ["<permission>"],
      "pending_permissions": []
    }
  },
  "seen": { "<anchor id>": { "root_version": 1, "epoch": 0, "catalogs": { "<source id>": 12 } } }
}
```

- `files` maps every file outside `data\` to its SHA-256. `footprint` lists OS changes to undo on
  remove (`{what, kind, path}`, kind `path | credential | shortcut | setting`).
- `source` and `anchor` name the module source and trust anchor it came from (§13). `sideload`
  is a source id reserved for developer mode.
- `seen` is anti-rollback state per anchor. It never goes down.
- A reader that meets an unknown `format` MUST NOT write the file.

`hideout.toml` (the hub's own state, written only by the hub):
- `[modules.<id>] enabled`;
- `autostart`, the main window's placement, theme;
- (v1.6) `[windows]`: per page, popped out or not, Keep on top and its places per screen layout
  (§2.5). Checked on its own (`config.windows`); a bad entry is dropped, never the file;
- `[[sources]]` and `[[trust.anchors]]` (§13);
- `[privacy]` sharing settings (§13.5);
- `safe_mode` flag (§7.10).

### 7.4 Steps

Every step can be run as `hideout modules <step> <id>` (`--json` for machine output). Steps stop at
the first failure, with its error id.

| Step | What happens |
|---|---|
| **install** | (1) Fetch the archive from a configured source's accepted catalog (§12, §13), or a local folder or zip (private source or developer mode). (2) SHA-256 MUST match the catalog entry; not yanked or revoked (`trust.*`). (3) Unpack to `.staging\`; refuse `..`, absolute paths and Windows-reserved names. (4) Exactly the entry's files with their hashes (`trust.files`). (5) Parse the manifest; `id`, `version`, `kind` and `visibility` MUST match the entry (`trust.mismatch`). (6) Check `hub` (`manifest.hub`). (7) Runtime: download every pin, check `sha256` and `size`, unpack. (8) Move `.staging\` to `<version>\` atomically. (9) Record it, not enabled, with `pending_permissions` set to its permissions. |
| **verify** | **Shallow, before every launch:** the version is in an accepted catalog of its source and not revoked (yanked still runs), and the folder holds exactly the signed files (`data\` skipped). For `adopted`: `run[0]` exists; at connect, the `version` in `initialize` MUST equal the registered manifest's `version`, or the instance is stopped (`trust.mismatch`, state `install-failed`). **Deep, on demand** (`--deep`): every file hash, and runtimes' unpacked files. Never changes anything. A failed shallow verify MUST stop the launch. |
| **enable** | Check `requires` and `api` (`lifecycle.*`). Show the permission dialog for `pending_permissions` (§6.4); stop if declined (`lifecycle.declined`). Record the grants in `inventory.json` (through `hideout-core`) and `enabled` in `hideout.toml`. Arm activation events. |
| **disable** | Drain and stop the instance (§7.6). Its leases, holds and channels end; its panels hide. Files, data, settings, secrets and grants stay. |
| **update** | Install the new version beside the old. If it adds permissions, wait (§6.4). Then hot swap (§7.6) at the right moment (§7.6, "when"). Record `previous`. |
| **rollback** | Only if the previous folder exists: hot swap back to it, shallow verify first. A revoked version is refused (`trust.revoked`). |
| **remove** | Stop it and end its Job Object. Delete its secrets, undo its `footprint`, delete its version folders, then `data\` per `[lifecycle] data` (`ask` without a terminal is `keep`). Remove the records. First-party modules shipped in the MSI offer **Turn off**, not Remove (UX7). |

**CLI exit codes:** 0 done; 1 refused (error id printed, `error.id` in `--json`); 2 usage; 3 needs
approval; 4 busy (lock held); 5 the hub isn't running (for steps that need it).

### 7.5 Launch and registration

LSP-style: capabilities and versions are exchanged once.

1. **Launch.** The hub shallow-verifies, then starts the exe in a Job Object (§7.7) with:
   - `HIDEOUT_PIPE`: the router pipe name, the one this hub serves (§8.2: a hub on another
     folder than the default serves a name of its own);
   - `HIDEOUT_TOKEN`: a one-time launch token;
   - `HIDEOUT_INSTANCE`: the instance id;
   - (v1.10, #983) `HIDEOUT_HUB_PID`: the hub's process id. Before it sends anything (the token
     least of all), a module checks that the pipe's server is that process
     (`GetNamedPipeServerProcessId`); the SDK's `connect_env` does. A launch without it (an older
     hub) skips the check.
2. **Connect.** The module sends `initialize {id, version, instance, token, hub, api}`. The hub
   checks the token, the pid and the exe hash. The result gives `granted`, `settings_dir`,
   `data_dir` and the served `hub` and `api` versions.
3. **Import** (handover only): the hub sends `state.import {blob, from_version}` if the old
   instance exported state.
4. **Register.** `module.register {methods, facts, channels}`, then `ui.register {…}`.
5. **Ready.** `module.ready {instance}`. Only now does the router send it traffic, and its panels
   go live.

A module that doesn't send `module.ready` within 30 s of start is treated as crashed.

### 7.6 Hot swap

An upgrade, a rollback, a reload or a dev rebuild. Four steps (design §4.5):

1. **Stage.** The new version is in its own folder and passes the launch check.
2. **Drain.** The hub sends `module.drain {reason}` to the old instance.
   - Answer `{state: "done"}`: go on.
   - Answer `{state: "busy", reason}` (a flash running, unsaved edits, a wizard open): the swap
     waits. The module's header shows "Update ready: waits until you finish <reason>", with a
     **Force** button. The hub sends `module.drain` again only on the module's `module.idle`
     notification or the user's Force, never on a timer (UX6).
   - After `done`, the old instance finishes calls in flight (5-second cap) and gets no new ones.
3. **Start or stop, by `swap` mode:**
   - **`overlap`** (no exclusive resource): start the new instance beside the old; on
     `module.ready`, route new calls to it; send `module.exit` to the old.
   - **`handover`** (an exclusive OS resource: Raw HID, LampArrays, the topmost overlay): call
     `state.export {}` on the old (result `{blob}`, at most 256 KiB, or `null`); send
     `module.exit`; start the new one; send it `state.import`.
   - Callers see at most one error 2 and retry. A key action is queued up to 2 s, delivered once,
     then dropped with a named status line ("Mic mute didn't run: Mic restarting") (UX3).
4. **Rebuild.** New panels replace old ones, keeping view state (§5.6). Facts are published
   again. The routing table changes in one step.

**When a swap may run (UX6):**
- Catalog updates apply when the module is idle (drain says `done`) and no game is running.
- Modules with `swap = "handover"` that own lights or the keyboard link update together, only at
  sign-in or with the hub's own update, never mid-session.
- During a planned swap the header says "Updating…" (blue) for at most a second, and input is held
  and replayed after `module.ready`. Only an unplanned exit shows "Restarting".

**Clarifications (v1.10, #983):**
- **Force** sends `module.drain {reason: "force"}` once and the swap goes on whatever it
  answers. **Not now** keeps the old version running; the new one runs at the module's next
  start, or when Review (for an update that adds permissions) is pressed later.
- An error answer to `module.drain` (no handler, the module gone, the call timed out) counts as
  `done`: there is nothing to wait for.
- After `module.exit` the old instance has 5 s to exit; then its Job Object is ended. Neither
  is a crash, and the new instance starts on its exit. The state stays `updating` from drain
  `done` until the new instance's `module.ready`; it reads `starting` at no point.
- Both `swap` modes stop the old instance before the new one starts for now (the supervisor
  runs one instance per module); `overlap` starting the new one beside the old is a later
  step. `handover`'s `state.export` / `state.import` is the router's.
- A crash while an update waits (for Review, a busy drain or Not now) restarts into the
  installed (new) version, which connects with the grants already given. For an update that
  adds permissions, the module reads Update waiting for you again once it is ready, and Review
  swaps it once more so it connects with the new grants.
- The game check in "when a swap may run" is the caller's: the update step calls the
  supervisor only once no game runs.
- The dev loop runs only in isolated lab hubs. A hub in developer mode shows a warning bar with
  **Leave developer mode**.

### 7.7 The supervisor

Design §3.1.

- **Contains** each instance in its own Job Object: memory and CPU caps, no breakaway, and an
  AppContainer when the permissions allow it.
- **Caps (v1.10)** come from the manifest's `[limits]`, each clamped to the hub's maximum
  (4,096 MiB for all the module's processes, a hard 50 % of the machine's CPU); an absent value
  is the maximum. A clamp is logged, not refused. A module taken back after a hub restart gets
  them again in its new job (§7.9).
- **AppContainer (v1.10, #983)** is opt-in for now: the hub setting `[supervisor] appcontainer =
  true` in `hideout.toml` (off when absent), until the first-party modules have run in one.
  With it on, the plan is made from the module's **granted** permissions (an ungranted one
  counts for nothing):
  - **Contained** (the usual case): its own AppContainer (`CreateAppContainerProfile`, profile
    `hideout.<id>`, or `hideout.<first 38 of the id>._<16 hex of its SHA-256>` when that would be
    longer than 64 characters; no id label starts with `_`, so the two forms never meet and two
    ids never share a profile; removed with the module). What the
    hub brokers (facts, lights, `net.connect`, `net.listen`, GPU, `secrets.ref`, app watches)
    needs nothing inside. A permission the module carries out itself through a device gets the
    matching capability in `SECURITY_CAPABILITIES`: `device.hid:` gives `humaninterfacedevice`.
  - **Outside**, logged: a granted `net` or `files`, whose permission lines already told the
    user it runs without the sandbox or with their files.
  - **Refused** (`install-failed`, "It can't run in a sandbox, and Hideout is set to sandbox
    modules."): a module whose plan can't be made (a grant that doesn't parse). Nothing runs
    without a sandbox unless the user approved something that says so.
  - **Contained too (#1092),** each for a reason that holds for any module of its kind:
    - **A stdio module** (§8.2's bridge). Its stdin and stdout are two anonymous pipes the hub
      makes (`CreatePipe`, neither end inheritable), and the child's ends and its log are the
      whole `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, made inheritable only while it starts, under
      the same spawn lock. An anonymous pipe has no name, so nothing else can open it, and an
      inherited handle needs no access check, so the container needs no ACE and no label for
      it. The bridge knows it by its handle (and records its pid and SID, for channels). It
      never opens the router pipe, so its SID is not a guest there and the pipe keeps its
      Medium label for it.
    - **`secrets`.** The hub keeps a module's secrets in Credential Manager and answers
      `secrets.get` and `secrets.set` (§4.7); the module never touches the store, which a
      container would block (rightly) if it tried.
    - **Direct channels** (`channel.open:`, `channel.offer:`, a declared `channel:`), by the
      rules in §8.4: a contained provider makes its pipe in its own namespace, and a contained
      consumer's SID is let into the provider's pipe.
    So the first-party bridged plugins run contained with the setting on: Broadcast Status
    (`net.connect`, `apps.watch`, `secrets`, all brokered), Focus timer (its state in
    `data\`), Mic mute (the hub does the Core Audio work) and OpenRGB (`net.listen`,
    brokered). The iCUE mirror runs **outside** because its user grants `files`: it loads
    Corsair's SDK DLL from wherever the user put it, and that SDK talks to iCUE over channels
    a container blocks.
  The container may read and run its version folder and write its `settings\` and `data\`,
  nothing else of the user's; stdin and stdout are NUL (a stdio module: its two pipes), stderr
  is its log; the three handles are inheritable only while it starts, with no other start of
  the supervisor meanwhile. The
  router pipe lets its SID in with read and write but not `FILE_APPEND_DATA` (on a pipe, the
  right to create an instance, so a contained module never serves the hub's pipe), and a Low
  mandatory label; a client opens the pipe with exactly those rights (`0x0012019B`; the SDK's
  `open_pipe` does), not `GENERIC_WRITE`. The hub changes the pipe's descriptor through its
  first instance, which it holds itself; the instances share it. A removed module's SID is
  taken off, and with no guest left the label is Medium again. A container that can't be set
  up fails the launch; it never runs the module uncontained instead. Taken back after a hub
  restart, a contained process keeps its container and is let in again only if it is its own
  profile's.
- **What stays open while modules are contained (v1.10, #983).** The Low label is per pipe, so
  a Low-integrity process of the user's that isn't a container (a browser's or PDF reader's
  sandbox) can open the router pipe and make instances of it. The router refuses it as a
  client. As a server it can catch a module's connect: a module that checks
  `HIDEOUT_HUB_PID` sends it nothing and fails to start (a denial of service, counted as a
  crash), and an outside tool that doesn't check the server could talk to it. Without the
  setting, or with no contained module running, the pipe keeps Windows' Medium floor and none
  of this applies. (#1092) A channel pipe that is Low (its consumer or its provider is
  contained) can likewise be connected to first by such a process: the provider admits only
  the consumer's pid, so that client gets nothing and the channel is spent (a denial of service,
  the consumer opens again).
- **Restarts** a crashed instance with backoff from 1 s up to 60 s. Five crashes in ten minutes
  stops it (one notification per module per sign-in). Exit code 0 is a stop, not a crash. A
  channel closed for an oversized line (§8.1, v1.2) counts as one crash.
- **Backoff steps (v1.1):** fixed delays of 1, 2, 4, 8, 16, 32 and 60 s for the 1st, 2nd, ... 7th
  restart in a run, then 60 s for every one after. No jitter: the delay is exactly the step.
- **A run (v1.8)** is the restarts since the ten-minute crash window was last empty. A crash with
  no other crash in the ten minutes before it starts a new run: its restart waits 1 s and it is
  crash 1 of the window. Start again, Start with defaults (§7.8) and a rollback by itself also
  start a new run and an empty window. There is no separate quiet timer: crashes closer than ten
  minutes apart keep one run going (its steps keep growing) even when the window never holds
  five, so it never stops.
- **Notices** a crash by events only: the process handle and the pipe closing. No heartbeats
  (#281).
- **Captures** each module's stderr to its log.
- **Does not** probe health, set resource policy for a domain, or restart a module because
  another needs it.
- **Outside problems are module states, not crashes (v1.4).** An outside app that is closed or
  missing, a file the user hasn't picked yet, a port another program holds, a device that is
  away: a module SHOULD keep running and say so in its own panels (a tile `value` with `tone`
  `attention` or `off`, and the page's How to connect steps), and try again on an event
  (`app:<name>.exe`, `apps.watch`, a `settings.set`, a fact) or on the user's **Try again**
  (`ui.event`). It MUST NOT exit non-zero for these: five such exits would read as "Stopped after 5
  crashes" for something the user can fix. It exits non-zero only on a real fault. Polling to find
  out when the problem clears follows the no-timers rule (§4.6): events first.

### 7.8 States (UX4)

One state list, used by the panel header, the Modules page and the tray status line. The hub
publishes it as the fact `module.<id>.state` (and `module.<id>.reason`).

Two more facts carry what the crash bar and the stopped card need (v1.4), published by the
supervisor beside the state:
- **`module.<id>.restart`:** `{delay_s, crash, of}` while the module is `restarting`: the seconds
  until the restart (a §7.7 backoff step), this crash's number in the ten-minute window (1 to 4)
  and the limit (`of`, 5). Drawn as "Restarting in <delay_s> s (<crash> of <of>)". Set to `null`
  when the restart starts. No other fields.
- **`module.<id>.recovery`:** while the module is `crash-stopped`, the step of the recovery ladder
  its card offers: `start-again` (the first stop this sign-in), `defaults` (the second),
  `go-back` (the third and later, with a previous version) or `last` (the third and later with
  no previous version: Copy diagnostics and Turn off <Module>). The card shows that step's one
  main button, then Copy diagnostics. `null` once the module starts again.
- **`module.<id>.previous`:** beside `recovery`, the version "Go back to <version>" names, or
  `null` when none is kept (#983).
- **`module.<id>.waiting` (v1.10):** while the module is `waiting`, the id of the root module
  its Waiting for <module> names and its **Go to <module>** opens (§4.4). `null` once it isn't
  waiting.
- **`module.<id>.failed`:** `launch` while the module is `install-failed` because launching it
  failed (the file check, or the pid or exe-hash check at `initialize`): it then reads "Couldn't
  start <Module>". `null` once it starts again (#983).
- **`module.<id>.revoked` (v1.6):** `{version, reason}` once an accepted catalog revoked the
  version that was running (§12.4): `reason` is the catalog's `reason` text, as its publisher
  wrote it (it may be empty). The hub stops the module and ends the leases it held (§8.5).
  `null` once another version of the module connects, or (v1.9) once the installed version is
  no longer withdrawn. The module's own words never reach it: `module.*` facts are the hub's.
  (v1.9) Also published for an installed version that isn't running (turned off, waiting for a
  call, or found at hub start), so its Modules row can say so.
- **`module.<id>.check` (v1.9):** while the module is `revoked` (or off with a withdrawn
  version), the card's **Check for updates**: `checking` while one fetch of its source's catalog
  runs, `none` when the source had nothing newer, `failed` when it couldn't be reached. `null`
  before the first check and once the version is no longer withdrawn. One fetch per press, no
  retry loop; a newer version that isn't withdrawn takes the normal update path (`updating`,
  `update-waiting`).
Both are read like any fact (`facts.read:module.*`); a reader that doesn't know them ignores them.
Values outside these shapes are a hub bug, not a module's.

| State id | UI name | Reason line | Ways out |
|---|---|---|---|
| `starting` | Starting… | — | none needed (cached view) |
| `running` | — | — | — |
| `updating` | Updating… | "Your place on this page is kept" | — |
| `update-busy` | Update ready | "Waits until you finish <reason>" | **Force**, Not now |
| `update-waiting` | Update waiting for you | the new permission, in plain words | **Review**, Not now (keeps the old version) |
| `restarting` | Restarting (n of 5) | when it stopped | Open log |
| `crash-stopped` | Stopped after 5 crashes | "crashed 5 times in 10 minutes" | **Start again**, **Start with default settings**, **Go back to <previous>**, Copy diagnostics |
| `waiting` | Waiting for <module> | the root cause, named once | **Go to <module>** |
| `needs-hub` | Needs a newer Hideout | — | **Update Hideout**, Go back to <previous> |
| `install-failed` | Couldn't install / update | plain cause | **Try again**, Open log |
| `quarantined` | <Module>'s page couldn't be shown | §5.5 | **Start again**, Go back to <previous> |
| `revoked` (v1.9) | Withdrawn ("<Module> <version> was withdrawn") | "Its publisher withdrew this version, so Hideout stopped it." + the publisher's note | **Check for updates**, Go back to <previous>, Turn off <Module>, Copy diagnostics |
| `off` | Off | — | Turn on |

Rules:
- **Withdrawn (`revoked`, v1.9):** the module is stopped (drain and stop, §7.6) and its files,
  data, settings, secrets and grants are kept, as for **disable** (§7.4); it stays enabled.
  Nothing starts it (no Start again, no Turn on; a Turn on from a file, MCP or the CLI is refused
  with `trust.revoked`) until another version is installed or **Go back to <previous>** runs.
  Go back is offered only when a previous version is kept and isn't revoked itself (a yanked
  one still runs, §12.4), and is always the user's click. **Turn off** disables it: the state is
  `off`, and the row reads "Off · <version> was withdrawn" with **Check for updates** instead of
  Turn on. The publisher's `reason` is drawn on the card only, as plain text (never markup, a URL
  never a link), labelled "Publisher's note:", with C0/C1 controls, bidi marks, overrides and
  isolates and zero-width characters dropped, whitespace runs as one space, and at most 160
  characters (cut on a whole character, "…"); never in the notification, tray, tile, tooltip or
  Modules row. Copy diagnostics has it in full, those characters escaped (`\u{202e}`). The
  notification comes once per withdrawn version (remembered in the hub's own state), not per
  sign-in; later sign-ins show the card and the tray's red dot only.
- **Start with default settings** renames the settings folder to `settings.bad-<time>` and starts
  the module. The hub needs no knowledge of the values.
- **Automatic rollback:** a module that reaches `crash-stopped` within 10 minutes of an update goes
  back to the previous version by itself and says so.
- **Root cause once:** a state caused by another module says "Waiting for <module>" and links
  there. The tray shows only the root.
- (v1.10) The tray draws no status line for a `waiting` module: the root's own line (its
  `restarting`, `crash-stopped` and so on) is the one. The `waiting` reason line names the
  root ("Example Link is not running."); the UI name is "Waiting for <root's name>".
- (v1.10) **needs-hub** is found at hub start (the installed manifest's `hub` isn't served, so
  it is read as `manifest.hub`) or at a launch. Nothing starts it: not hub start, an
  activation, Start again or Turn on. **Update Hideout** is the hub's own update (a newer hub
  starts it as usual); **Go back to <previous>** rolls back, then starts it, and is offered
  only with a previous version kept (`module.<id>.previous`).
- (v1.10 clarification, #1096) A `needs-hub` module is **listed, never dropped**: the hub reads
  its manifest's head (§4.1: `id`, `name`, `version`, `hub`) and names it by that `name` on the
  Modules page and in the tray. This holds for every source: an installed catalog or sideloaded
  version (the hub went back a version) and a lab module (docs/hideout-lab.md), which joins the
  lab inventory from its head with nothing granted. Installing or updating to a version whose
  `hub` isn't served is still refused at install step (6) (`manifest.hub`, §7.4) before
  anything changes, so the version that runs stays; that is why `needs-hub` arises only at hub
  start or a launch, and why no "update that caused it" notification exists.
- (v1.11) **Update Hideout** opens the page of the release the module needs: the
  `min_hub_release.url` of the installed version's entry in its own source's accepted catalog
  (§12.4), only when that `url` is allowed (`https://github.com/BanditF/foxglove/releases`, or a
  path below it made of `/` and `A–Z a–z 0–9 . _ ~ + -`, no empty segment and no `..`, at most
  512 bytes). Otherwise, and for a sideload or lab module, it opens
  `https://github.com/BanditF/foxglove/releases`. The hub never opens any other link from a
  catalog. The tray's Update Hideout names no module and opens the releases page.

### 7.9 When the hub goes

- Modules notice the pipe close and keep running for a **30-second grace period**, set once.
- The hub restarts itself after a panic or a GPU reset. It reads `run\instances.json` and takes
  back running modules: each reconnects with `initialize` and its original token; the hub checks
  pid and exe hash.
- **Taking back (#986).** Each record in `run\instances.json` holds the pid, the process's start
  time, the exe path and its SHA-256, and the launch token. The restarted hub takes back a process
  only when all of them match: the pid is open, its start time is the recorded one (else Windows
  gave the pid to another process, which is left alone), and it runs the recorded exe with the
  recorded hash. A process whose pid and start time match but whose exe doesn't is ended, that
  pid only; a survivor of a module no longer enabled, or of a stdio module (its pipes went with
  the hub), is ended the same way. The hub never ends a process by name. While a module is taken
  back, or its grace period runs, the hub starts no second instance of it, and the restart shows
  no crash bar, notification or crash count.
- **Job Objects across a restart.** Jobs are made without kill-on-close, so the hub going doesn't
  end its modules (they need the grace period). Only the hub holds a job's handle, so the job's
  name goes with the hub; its processes stay inside it, caps included. The restarted hub puts a
  survivor in a fresh job, nested in the old one (Windows 8 and later), and ends it through that
  job. Processes the module started before the restart stay only in the old job. If Windows
  refuses the fresh job, the hub tracks the process by its handle and logs that it has no job.
  Keeping the old job by a second holder process was rejected: one more process to supervise for
  no gain in containment.
- **One supervisor per hub folder.** A second hub on the same folder only shows the first one's
  window; it starts neither router nor supervisor, so it never starts or takes back a module.
- With no hub back within the grace period, each module exits cleanly (lights handed back, the
  Info screen handed back to the firmware).
- Pop-outs reopen at their places (§2.5).

### 7.10 Safe mode and bisect (UX5)

- After three hub restarts in five minutes the hub starts in **safe mode**: only the Modules page,
  no module started, a banner with **Start normally**. Safe mode is a flag the hub sets for itself
  in `hideout.toml`, logged, cleared by **Start normally**. A restart counts when the last run
  ended without a clean exit: the running hub's mark in `run\hub-run.json` is still there.
  Quit Hideout, `WM_ENDSESSION` and a console Ctrl+C or close clear it; a second hub on the same
  folder never touches it (#1033).
- **Find the module that breaks Hideout** (bisect, from safe mode; clarification, #1033,
  `docs/hideout-safe-mode-ux.md`): the hub starts half of the modules **Start normally** would
  start (a module that `requires` another goes with it, as one entry) and halves again. A round
  ends **closed** when the hub closes without a clean exit (the mark above) or the user says It
  closed, and **fine** after 5 minutes open with the PC awake or on It's fine. After a close the
  hub starts with nothing on, and the next round starts only on a click. A clean exit pauses the
  round; an end the hub can't read (no readable `run\hub-run.json`, or Windows started after
  that run did) asks whether it closed by itself. A module on alone in a round that closed is
  named, with Turn off <Module>, Copy diagnostics, Start normally anyway, and Go back to
  <previous> when one is kept; every round fine names none, a close with nothing on names Hideout
  itself. Progress lives in `run\bisect.json` and survives restarts; the result's choice or Stop
  finding clears it, and only Turn off or Go back changes what is saved. The 3-in-5-minutes rule
  is off while a search runs.
- **Bar note (#1033):** a search shows as a bar at the top of Hideout's pages (a landmark region,
  first in tab order) and in the tray's status line and menu, never as a window of its own.

## 8. IPC

### 8.1 Wire and error codes

- **JSON-RPC 2.0**, one UTF-8 message per line, at most 256 KiB. Foxglove's v1 wire, unchanged.
- Requests have an `id` and get exactly one response. Notifications have none.
- `initialize` comes first on every connection; anything else first answers error 5.
- **Error codes:** JSON-RPC's own (−32700, −32600, −32601, −32602, −32603), then 1 unsupported,
  2 disconnected (provider gone or swapping), 3 busy, 4 timeout, 5 denied, 6 backend, 7 version,
  8 rate-limited. New codes only with a `hub` minor. A refusal's `data.reason` is plain words for
  the UI.
- A method no module provides answers −32601. A provided method whose module isn't running answers
  2 (after activation by `call`, if declared).
- A call with no answer fails after 30 s on the caller's side (error 4). A result that arrives
  after that is dropped, and the id is free again.
- **Request ids (v1.2):** an `id` is unique among the requests in flight on one connection. A
  request whose id is already in flight is not routed. The hub answers it with error -32600,
  `data.id` `rpc.duplicate-request-id`, and **id null** (an echoed id would be taken for the first
  call's answer). The first call is unaffected and gets its own response. An id may be reused once
  its response (a timeout included) has been sent.
- **Oversized line (v1.2):** the hub does not buffer past 256 KiB (262,144 bytes). It sends error
  -32600, `data.id` `rpc.frame-too-large`, id null, then closes that channel. For a module's
  channel the supervisor counts it as one crash toward the crash bar (§7.7); an outside tool is
  just disconnected.
- A line that isn't valid JSON answers -32700 with id null.

### 8.2 The router pipe and who connects

- **`\\.\pipe\hideout-<user>`**, `<user>` = `%USERNAME%` keeping ASCII letters, digits, `-`, `_`.
  Current-user ACL, `PIPE_REJECT_REMOTE_CLIENTS`.
- **One name per hub folder (clarification, #1010).** The name above is the default hub
  folder's (`%LOCALAPPDATA%\Hideout`), unchanged. A hub on any other folder (`HIDEOUT_CONFIG_DIR`
  or `HIDEOUT_HOME`, a lab or test hub) appends `<suffix>` = `.` and 16 lower-case hex digits:
  FNV-1a 64 of the folder's key, its canonical path when it exists (else its absolute path),
  without `\\?\`, lower case, `/` separators, no trailing `/`. So a lab hub serves
  `\\.\pipe\hideout-<user><suffix>` and never meets the user's hub. An override that names the
  default folder is the default hub, with no suffix. The same `<user><suffix>` names the hub's
  other per-user objects: the `foxglove-<user><suffix>` alias, the lab-only
  `hideout-capture-<user><suffix>` (#1008) and `hideout-lab-<user><suffix>` (#1014) pipes, the
  single-instance and supervisor locks, and the modules' Job Objects. A module never derives
  the name: it opens `HIDEOUT_PIPE` (§7.5), which the hub sets to the pipe it serves. An
  outside tool that wants a lab hub is given that hub's name.
- **Modules** connect with their launch token.
- **Outside tools** (`foxglove-cli`, the `hideout` CLI, `hideout mcp`, scripts) connect without a
  token and are approved once each.
- **Legacy stdio plugins** (`transport = "stdio"`) are bridged by the hub.
- **The bridge carries panels (v1.4).** A bridged connection is the module's connection: the
  hub reads its identity from the bridge, not a token, and treats its messages as any module's.
  `ui.register` and `ui.patch` from the plugin, and `ui.event`, `action.invoke`, `settings.set`
  and `ui.visibility` to it, pass unchanged. A bridged plugin MAY skip `module.ready`; calls to it
  are delivered without it, and the hub counts it ready once its `initialize` is answered, so
  the 30 s ready timer (§7.5) never ends it (clarification, #1001). Its panels are the
  `[[ui.panels]]` of its `module.toml` (§4.10).
- **Client identity (v1.3).** What an outside tool is comes from the signed catalog, not from
  what it calls itself. An MCP client is a catalog entry with `kind = "mcp-client"`; the
  `mcp:<client>` name prefix (§9) is a lab-only fallback until the catalog lists clients (H2.2).
  The first-party `hideout` CLI, the only client that may hold `modules.manage` without asking,
  is identified by its exe hash in the signed root catalog, checked at `initialize`.
- **Who checks `initialize` (v1.3).** The pid and exe-hash check at `initialize` (§7.5) is the
  supervisor's (H2.5), which started or recognises the process, not the panel host's. Until the
  permission dialog exists, routers keep the lab-only behaviour: approval by name.
- The old `\\.\pipe\foxglove-<user>` is served as an alias once `foxglove.exe` no longer owns it
  (§8.6).
- **No network listener** in the hub. Loopback WebSockets stay opt-in adapters inside modules,
  with a per-install token. Tailnet exposure is Vulpes-only and MUST NOT forward to the hub's
  pipe.

### 8.3 Routing (control plane)

- **Methods** go to the module that provides them (§6.3). Foxglove v1 names keep their meaning
  (`lighting.*` → `hideout.lights`, `screens.*` → `foxglove.screens`, `art.*` → `foxglove.art`),
  so existing plugins and `foxglove-cli` work unchanged.
- **Facts** go through the hub's cache: modules publish with `facts.set`, subscribers and panels
  get `facts.changed`.
- The hub carries control and status only. Fast data uses channels.

### 8.4 Direct channels (data plane)

- A provider declares channels (`provides = ["channel:<name>"]`, `channel.offer:<name>`).
- A consumer calls `channel.open {module, channel}`. The hub checks `channel.open:<module>/<channel>`,
  then calls `channel.create {channel, consumer, nonce}` on the provider. The provider creates a
  one-time named pipe with a current-user ACL and answers `{pipe}`.
- The hub returns `{pipe, nonce}` to the consumer and steps out.
- The consumer connects and sends the nonce first. Then frames flow directly:
  `u32 little-endian length` + payload. Either side closing ends the channel.
- **The channel's wire (v1.4, #977).** Written down from the first SDKs; no new message.
  - **Pipe name.** A provider answers `channel.create` with `\\.\pipe\hideout-ch-<rest>`:
    exactly that prefix (the local machine, lower-case `pipe`), then one or more ASCII letters,
    digits, `.`, `_` or `-`, at most 256 characters in all (a contained provider: the second
    form, below, v1.10). The SDKs make
    `hideout-ch-<pid>-<n>-<module>-<channel>`. The hub checks the answer before passing it on:
    any other `pipe`, or none, answers the consumer error 6 (backend) with `data.id`
    `channel.pipe`, so a consumer is never sent to another machine's pipe or to the router's.
    A consumer MUST NOT connect to a name that fails this rule, and connects with
    identification-level impersonation only.
  - **One time.** The pipe is created with one instance, the first-instance flag, remote
    clients rejected, and a current-user-only DACL (one allow ACE, the user's SID; as the router
    pipe, §8.2). It takes one client ever: the provider never makes a second instance, so a
    second connect fails (pipe busy while the first holds it, not found after). A new channel
    needs a new `channel.open`.
  - **Nonce.** The consumer's first frame is the nonce's UTF-8 bytes (the hub's nonces are 32
    lower-case hex digits). The provider reads at most 256 bytes for it and compares in
    constant time. A missing, longer or different first frame closes the pipe at once, with no
    answer, and the channel is spent. Nothing acknowledges a good nonce: frames simply flow.
  - **Deadline.** The provider waits at most 30 s (the call timeout, §8.1) from its
    `channel.create` answer for the consumer to connect and send the nonce, then closes the
    pipe.
- **Who the pipe is for (v1.10, #1092).** Additive fields; a side that doesn't send them is
  treated as before (the nonce decides).
  - `channel.create` carries `consumer_pid` (the consumer's process id when the hub knows it:
    the router pipe's client, a bridged child, or the hub itself for its panel host) and, when
    the consumer runs in an AppContainer (§7.7), `consumer_sid` (its SID, from its token).
  - The provider's pipe lets `consumer_sid` in, and only it: an allow ACE with client rights
    (`0x0012019B`, no create-instance) and a Low mandatory label, which an AppContainer needs to
    open it. Without `consumer_sid` the DACL is the user's alone and no label is set.
  - On connect, before it reads a byte, the provider admits only `consumer_pid`
    (`GetNamedPipeClientProcessId`), or without one only a client whose token has
    `consumer_sid` (a token it can't read is a refusal). Anyone else is dropped and the
    channel is spent, as for a wrong nonce.
  - The grant (`channel.open`'s result, the panel host's answer) carries `pid`, the provider's
    process id when the hub knows it. The consumer opens the pipe with client rights only and
    identification-level impersonation, and sends nothing, the nonce least of all, unless the
    pipe's server is that process (`GetNamedPipeServerProcessId`), as a module checks
    `HIDEOUT_HUB_PID` (§7.5). The SDK's `connect_to` and the panel host do.
  - **A contained provider's pipe.** An AppContainer may make pipes only in its own
    namespace: it creates `\\.\pipe\LOCAL\hideout-ch-<rest>`, which every other process opens
    as `\\.\pipe\Sessions\<session>\AppContainerNamedObjects\<its SID>\hideout-ch-<rest>`, and
    answers `channel.create` with that full name (at most 256 characters; the SDK cuts the
    end of `<rest>`). This is the second form of a channel pipe name: the session in digits
    (no leading zero), the SID `S-1-15-2-` and dash-separated digits, then exactly one segment,
    `hideout-ch-` and `<rest>` by the rule above. Its pipe is Low by Windows' rule (made by a
    Low process).
  - **The whole name, strictly (#1092 review).** Both forms are parsed in full: the exact
    prefix, then exactly one segment. No further `\` or any `/`, no `.` or `..` segment, and
    `<rest>` doesn't end in `.` (Windows drops a trailing dot). Nothing Windows could normalise
    into another path passes, a remote UNC share least of all (opening one would send the
    user's credentials). A second-form name counts only in the reader's own session: the hub
    checks its session, and a consumer checks its own. As defence in depth, a consumer opens
    only a name that `GetFullPathNameW` leaves unchanged under `\\.\pipe\`, at identification
    level (`SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`). The vectors' table states the
    reader's session (`"session": 1`).
  - **The router's check.** A contained provider's answer passes only in the second form with
    its own SID (from its token) in the hub's session, and a provider outside a container only
    in the first form: no provider can send a consumer to another module's pipe.
  - **Frames.** Both directions may carry frames; what they mean is the channel's own
    business. A payload may be empty and is at most 64 MiB; a longer length prefix closes the
    channel unread. A stream that stops inside a length prefix or a payload ends the channel;
    the whole frames before it count. Panel stream frames (below) are payloads with their own
    4 MiB cap.
- **Panel stream frames** (§5.4) are binary, never JSON. Each payload is a 12-byte header and a
  body. Header, little-endian: `u8 kind`, `u8 flags` (0), `u16 width_or_count`, `u16 height`
  (0 unless `kind` 1), `u16 reserved` (0), `u32 seq`.
  - `kind` 1, **image:** raw RGBA, 8 bits per channel, `width × height × 4` bytes, row-major.
  - `kind` 2, **canvas (packed LED array):** `count × 4` bytes of RGBA, one per shape in the
    order the canvas declared them.
  - `kind` 3, **text:** UTF-8 bytes to append. `width_or_count` (v1.1) is the UTF-8 byte length
    of the text body; `height` is 0. A mismatch drops the frame like any other length mismatch.
    (v1.5) A text frame MUST end on a character boundary: a sender splits long text between
    characters, never inside one. A body that isn't whole UTF-8 is dropped like any bad frame
    (as v1.1 readers already did) and counted in the log, never drawn. The same frames feed a
    `markdown` stream.
  - A frame whose body length doesn't match its header is dropped.
- The hub's panel host opens streams the same way (§5.4) and is just a channel end.
- Realtime paths (scene to keyboard LEDs, key events to the overlay, art pushes, LED wizard
  frames, live previews) are channels; the hub only grants them (design §4.6 table).

### 8.5 Connections, leases and liveness

- Anything a connection holds ends when it ends: lighting leases, Info screen holds, GPU tenancy,
  subscriptions, app watches, channels.
- Liveness is the connection, not a heartbeat.
- **Leases (v1.3).** A lease is an id (`lease`) a provider hands out in a call's result, for
  example `lighting.lease {...}` answering `{lease}`. The hub records the connection that made
  the call as the lease's **holder**, keyed by provider and lease id.
- **`lease.closed` (v1.3).** When a lease ends, the hub sends its provider the notification
  `lease.closed {lease, reason}` so it can drop the state behind it. `reason` is
  `"disconnect"` (the holder's connection ended), `"released"` (the holder gave it back with the
  provider's release call, a method named `<family>.release` with `lease` in its params; the hub
  forwards that call first, then sends `lease.closed`) or `"revoked"` (the hub ended it, for
  example when the holder's permission was withdrawn). Each lease sends it once. A provider
  whose own connection ended gets nothing and its leases are forgotten with it.
- **Revoked holders (v1.6).** When an accepted catalog revokes the running version of a module
  that holds leases, the hub ends each of them with `reason: "revoked"` before it stops the
  module; the disconnect that follows sends nothing more for them.
- **Addressed events (v1.3).** A provider event whose params carry `to: <lease>` is delivered
  only to that lease's holder, whatever permissions other connections hold. An event to a lease
  the hub does not know (ended, never handed out, or another provider's) is dropped. Events
  without `to` are broadcast to the holders of the event's permission, as before.

### 8.6 Discovery

- **There is one endpoint.** A client finds the hub by opening its fixed pipe; if it isn't there,
  the hub isn't running.
- A module learns that a provider is there from the `module.<id>.state` facts, never by polling.
- Modules come only from manifests in configured module sources (§13.4). The hub has no built-in
  list.
- **One-owner rule (pick G):** at start, `foxglove.exe` and `hideout.exe` each try the other's
  pipe. If the other answers, the starting app asks it to show its window, then exits. "Go back to
  Foxglove" in the tray restores Foxglove's Run value and Start entry.

### 8.7 Versioning

- **`hub`** (this contract, `MAJOR.MINOR`): manifests, the inventory, catalogs, the panel
  vocabulary and the hub messages below. Spec versions and `hub` move separately: spec 1.0 to 1.3
  are `hub` 1.0; spec 1.4 is `hub` 1.1 (a new setting kind and key), and so is spec 1.5 (only
  optional panel props and stream rules, no manifest key), and so is spec 1.6 (hub window rules
and a hub-owned `hideout.toml` table, no message or manifest key). The hub answers `initialize`
  with its own `hub`. (v1.10) Spec 1.7 to 1.9 stay `hub` 1.1; spec 1.10 is `hub` 1.2 (the
  manifest key `[limits]`). A hub 1.2 reader accepts `1.0`, `1.1` and `1.2`. (v1.11) Spec
  1.11 stays `hub` 1.2: its one addition is an optional catalog entry field, which a reader
  that doesn't know it ignores (catalog entries ignore unknown fields, §12.4).
- **Method-family APIs** (`api`): Foxglove's local API (1.13 today), Vulpes's (1.0 when built).
  A provider declares its API version in `module.register`.
- A minor only adds (methods, optional fields, events, manifest keys, node types, permissions).
  Unknown fields in messages are ignored (not in manifests, §4.1).
- A major breaks. The hub serves N and N−1 for one release cycle.
- "Served" = same major, server minor ≥ client minor.
- Schemas are generated from the Rust types (`schema/hideout-v1.json`) with pydantic models for
  Python, and a golden drift test.

### 8.8 Message reference

Every message named in this spec. "M" = module, "H" = hub, "T" = outside tool, "C" = any client.

**Lifecycle**

| Message | From → to | Kind | Params | Result | Needs |
|---|---|---|---|---|---|
| `initialize` | M → H | call | `{id, version, instance, token, hub, api}` | `{hub, api, granted, settings_dir, data_dir, safe_mode}` | a valid launch token |
| `initialize` | T → H | call | `{client: {name, version}, hub?, api, permissions}` | `{hub, app, api, granted}` | approval once per tool |
| `ping` | C → H | call | `{}` | `{}` | — |
| `module.register` | M → H | call | `{methods: [pattern], facts: [pattern], channels: [name], apis: {family: "MAJOR.MINOR"}}` | `{rejected: [{item, error}]}` | `provide:` and `channel.offer:` for each; catalog (§6.3) |
| `module.ready` | M → H | notification | `{instance}` | — | — |
| `module.drain` | H → M | call | `{reason: "update" \| "rollback" \| "reload" \| "disable" \| "remove"}` | `{state: "done"}` or `{state: "busy", reason}` | — |
| `module.idle` | M → H | notification | `{}` | — | — (after a `busy` drain) |
| `state.export` | H → M | call | `{}` | `{blob}` (≤ 256 KiB, base64) or `null` | — |
| `state.import` | H → M | call | `{blob, from_version}` | `{}` | — |
| `module.exit` | H → M | notification | `{reason}` | — (exit 0 within 5 s, or the Job ends it) | — |
| `lease.closed` (v1.3) | H → M | notification | `{lease, reason: "disconnect" \| "released" \| "revoked"}` | — | — |

**Panels and UI**

| Message | From → to | Kind | Params | Result | Needs |
|---|---|---|---|---|---|
| `ui.register` | M → H | call | `{instance, panels: [{id, slot, title, icon, tree}]}` | `{rejected: [{panel, error}]}` | panels declared in `[[ui.panels]]` |
| `ui.patch` | M → H | notification | `{panel, ops: [{op: "set" \| "insert" \| "remove", node, props?, index?, tree?}]}` | — | — |
| `ui.event` | H → M | call | `{panel, node, kind, value}` | `{}`, or an error with `data.reason` (refused) | — |
| `ui.asset.put` | M → H | call | `{hash, mime, data}` (base64, ≤ 256 KiB) | `{}`; (v1.7) the panel host's refusal as -32602 with `data.id` (§5.3) | — |
| `ui.window.open` | H → M | call | `{window}` | `{}` | `ui.window` |
| `ui.badge` (v1.3) | M → H | notification | `{panel, badge: "none" \| "attention" \| "dirty" \| "count", count?}` (`count` ≥ 1 with `count`) | — | the panel is the module's |
| `ui.visibility` (v1.3) | H → M | notification | `{panel, state: "visible" \| "hidden" \| "closed"}` (v1.4: `page` and `tile` panels) | — | — |
| `action.invoke` | H → M | call | `{action, source: "key" \| "tray" \| "palette" \| "panel"}` (v1.4: `panel` for a button with `action`) | `{}` | — |
| `settings.get` | H → M | call | `{}` | `{values: {name: value}}` | — |
| `settings.set` | H → M | call | `{name, value}` | `{}`, or an error with `data.reason` | — |
| `hub.notify` | M → H | call | `{title, body, level: "info" \| "warn" \| "error"}` | `{}` | `hub.notify` |

**Facts and channels**

| Message | From → to | Kind | Params | Result | Needs |
|---|---|---|---|---|---|
| `facts.get`, `facts.list`, `facts.subscribe`, `facts.unsubscribe` | C → H | call | as Foxglove API v1 | as v1 | `facts.read:<pattern>`; (v1.7) `apps.watch:<name>.exe` for that app's `apps.*` facts (§4.6) |
| `facts.set` | M → H | call | as v1 | as v1 | `facts.write:<pattern>`, or `facts:` in `provides` |
| `facts.changed` | H → C | notification | as v1 | — | the subscription |
| `channel.open` | C → H | call | `{module, channel}` | `{pipe, nonce}` | `channel.open:<module>/<channel>` |
| `channel.create` | H → M | call | `{channel, consumer, nonce}` | `{pipe}` | the provider offers it |

**Module management**

| Message | From → to | Kind | Params | Result | Needs |
|---|---|---|---|---|---|
| `modules.list` | C → H | call | `{}` | `{modules: [{id, name, version, kind, visibility, source, enabled, state}]}` | `modules.manage` |
| `modules.enable` | C → H | call | `{id}` | `{}` | `modules.manage` |
| `modules.disable` | C → H | call | `{id}` | `{}` | `modules.manage` |
| `modules.reload` | C → H | call | `{id}` | `{}` | `modules.manage` |

**Addressed provider events (v1.3).** Any provider event may carry `to: <lease>` in its params
(§8.5); the hub delivers it only to that lease's holder.

**Facts the hub publishes:** `module.<id>.state`, `module.<id>.reason`, `module.<id>.version`,
(v1.4) `module.<id>.restart` and `module.<id>.recovery` (§7.8), (v1.6) `module.<id>.revoked` (§7.8), `hub.safe_mode`. Facts about private modules never reach MCP or the tailnet by default (§13.5).

**Provider methods** (lighting, screens, art, GPU and the rest) are the providers' own API and
are listed in their API docs (local-api.md for Foxglove; §8.9 for the GPU).

### 8.9 Vulpes GPU methods

Provided by the Vulpes module.

| Method | Kind | Params | Needs |
|---|---|---|---|
| `gpu.status` | call | `{}` | `gpu.status` |
| `gpu.request` | call | `{class, why}` | `gpu.request:<class>`; error 3 while reserved for gaming |
| `gpu.release` | call | `{}` | the tenancy; also ends with the connection |
| `gpu.changed` | event | `{tenant, vram, reserved}` | `gpu.status` |
| `ai.ask` | call | `{agent, prompt}` | `ai.ask:<agent>` |

- A gaming reservation is never a method. It changes only through the user's own click on Vulpes's
  Free GPU action or panel button (`action.invoke` or `ui.event` with a user source).
- Gaming is a shared signal: Vulpes publishes `gpu.reserved`, the link publishes
  `keyboard.game_mode`; neither acts on the other's alone.

## 9. The MCP bridge

Design §4.7.

- **`hideout mcp`** is a stdio adapter and an outside tool of the router, connecting as
  `mcp:<client>` (`<client>` = the MCP client's `clientInfo.name`, ASCII letters, digits, `-`,
  `_`, `.`, at most 64). The user approves each agent client once.
- It exposes the hub's status and every enabled module's `[[mcp.tools]]`:

| Key | Type | Meaning |
|---|---|---|
| `name` | string | tool name, unique in the module; exposed as `<module name>_<name>` |
| `description` | string | for the agent |
| `method` | string | one method the module provides; its schema is the tool's input. A method the module's `provides` doesn't cover is refused (`manifest.mcp-method`, v1.1) |
| `access` | string | `read` or `write` |
| `ttl_max_s` | integer | write tools that hold something: the longest hold, at most 600 |

- **Write tools are time-limited:** leases have a TTL (≤ 600 s); holding the Info screen lasts at
  most 60 s, and a hold with no end is refused. The adapter releases at the TTL, at session end or
  when it exits.
- **Never over MCP:** the never-list (§6.6), every `keys`-class channel, and module lifecycle and
  grants (no tool installs, enables, disables or approves anything).
- **Private modules** (§13.5): their tools and status are left out unless the user turns on
  "Show private modules to AI agents" in `[privacy]`.
- **Vulpes keeps its own MCP server** (`arete mcp`, later `vulpes mcp`), so Claude Code and LM
  Studio work without the hub. Vulpes agents drive the desk through `hideout mcp` with their
  `mcp_client`.
- **No loop:** `hideout mcp` MUST NOT re-export Vulpes's own MCP tools.

## 10. Lab and dev loop

- The dev loop (rebuild one module, hot swap, no hub restart) runs only in isolated lab hubs with
  their own `HIDEOUT_HOME` and pipe name suffix.
- How a lab hub loads the first-party modules as they ship, without a catalog and without
  touching trust (lab mode, `<config>\lab-modules\`): [hideout-lab.md](hideout-lab.md). Hub
  behaviour only; no wire or manifest change.
- Latency of direct channels and the Dynamic Lighting gap during a lights handover are lab cases
  before any "no flicker" claim (UX6).

## 11. Scope

- The hub adds **composition only**: the registry, panels, routing, grants and supervision.
  Domain logic stays in modules.
- **Systems, not products:** modules are written against a class (any HID LampArray, any Raw HID
  keyboard speaking the link, any GPU tenant, any MCP client). Models and vendor quirks are content
  and catalog data, never code.
- **Licences:** the contract, the SDKs and every module talk over IPC and MAY use any licence the
  author picks; first-party SDKs are MIT. The hub is MIT too, written clean-room from this spec and the design
  doc; it never uses or adapts code from `app/` (GPL-3.0), and hub crates depend only on MIT or
  Apache-2.0 code.

## 12. Trust

### 12.1 Three kinds of trust, kept apart

1. **Module provenance** (this section and §13.6): minisign chains from configured trust anchors.
   Works without Authenticode.
2. **The hub's own update:** its own Ed25519 `latest.json` (Foxglove's). Not covered here.
3. **Code signing** (Authenticode, #383): SmartScreen and package identity. No part of the module
   system waits on it.

### 12.2 The Hideout root chain

```
Hideout root key (offline, minisign; public key pinned in hideout.exe)
  └─ signs root.json (format hideout-root-v1)
       └─ names catalog keys, each bound to ONE family and a list of id scopes
            └─ each family's key signs that family's catalog.json
                 └─ pins each module version: archive SHA-256 and every file's SHA-256
                      └─ a runtime's pins, inside its signed module.toml
```

- **One root** (kept pick). Foxglove's `foxglove-plugin-root` format stays readable as a root with
  one family, `foxglove`, so existing test catalogs verify. It MUST NOT be pinned in a release once
  the Hideout root exists.
- Signatures are minisign (Ed25519) over the exact file bytes, in a `.minisig` beside it.
- Times are RFC 3339 UTC, `YYYY-MM-DDTHH:MM:SSZ`, compared as text.

### 12.3 `root.json`

```json
{
  "format": "hideout-root-v1",
  "version": 1,
  "epoch": 0,
  "expires": "2027-11-09T00:00:00Z",
  "families": {
    "foxglove":  { "keys": ["RWQ..."], "scope": ["foxglove.*", "org.foxglove.*"] },
    "vulpes":    { "keys": ["RWQ..."], "scope": ["vulpes", "vulpes.*", "arete.*"] },
    "digitalis": { "keys": ["RWQ..."], "scope": ["digitalis", "digitalis.*"] },
    "hideout":   { "keys": ["RWQ..."], "scope": ["hideout.*"] }
  }
}
```

- A scope entry is an exact id or a prefix ending in `.*`. `*` alone is refused.
- Third-party public modules are listed in a first-party family's catalog under a scope the root
  grants that family.
- Scopes MUST NOT overlap between families (`trust.scope-overlap`). A key appears in one family
  only (`trust.key-reuse`).
- Checked against the pinned key, `expires`, and `seen` (`trust.root-expired`,
  `trust.root-rollback`).

### 12.4 A catalog (`catalog.json`)

Format `hideout-catalog-v1` (Foxglove's `foxglove-plugin-catalog-v2` plus). `family` is optional
(v1.1): a catalog signed by a user anchor (§13.6) has none, and a reader treats an absent `family`
as family `user`.

```json
{
  "format": "hideout-catalog-v1",
  "family": "foxglove",
  "epoch": 0,
  "sequence": 3,
  "expires": "2026-12-09T00:00:00Z",
  "modules": [
    {
      "id": "<id>",
      "version": "0.4.0",
      "kind": "service",
      "visibility": "public",
      "channel": "stable",
      "hub": "1.0",
      "sha256": "<archive sha256>",
      "url": "https://…/<id>-0.4.0.zip",
      "size": 2048,
      "files": { "module.toml": "<sha256>" },
      "provide": ["art.*"],
      "channels": { "<name>": { "capability": "<plain words>", "risk": "normal" } },
      "min_hub_release": {
        "version": "0.4.0",
        "url": "https://github.com/BanditF/foxglove/releases/tag/hideout-v0.4.0"
      }
    }
  ],
  "revocations": [ { "id": "<id>", "versions": ["0.1.0"], "action": "yank", "reason": "<why>" } ]
}
```

Accepted only if:
- the signature verifies with a key the anchor lists for `family` (`trust.signature`);
- every entry and revocation id is in that family's scope (`trust.scope`); one out of scope rejects
  the catalog;
- `epoch` equals the root's (`trust.epoch`); not expired (`trust.expired`);
- `sequence` is not below `seen` (`trust.rollback`).

Entries:
- `kind` and `visibility` MUST equal the manifest's (`trust.mismatch`).
- `provide` is the method patterns the module may register (§6.3).
- `channels` gives each offered channel its capability line and risk class (§6.2).
- (v1.11) `min_hub_release` (optional) names the first Hideout release that serves this
  version: `version` (`MAJOR.MINOR` or `MAJOR.MINOR.PATCH`, an optional `-` suffix of
  `A–Z a–z 0–9 .`, at most 32 bytes) and `url`, that release's page. It is what **Update
  Hideout** opens (§7.8), and only when the `url` is allowed there. A publisher sets it when the
  entry's `hub` is newer than some released Hideout serves. A value that isn't an object with both, or a bad
  `version`, is ignored (as if absent); it never rejects the entry or the catalog. Unknown
  fields in an entry are ignored, so an older reader skips it.
- `plugins` and `kits` are read as aliases of `modules`.
- yank: no new installs; revoke: also stops running.

After accepting, the reader stores the new `seen` before using the catalog; if it can't, it MUST
NOT go on.

### 12.5 What verification proves

- An archive installs only if its SHA-256 is in an accepted catalog whose anchor and scope cover
  its id, and that version isn't withdrawn.
- A module runs only if its folder holds exactly the signed files and it isn't revoked.
- None of this grants a permission (§6.5).

### 12.6 Developer mode (unsigned sideload)

- Outside any catalog, a module installs only in developer mode (`FOXGLOVE_DEV_UNSIGNED_PLUGINS`,
  `HIDEOUT_DEV_UNSIGNED=1`). Source id `sideload`. Without developer mode the install is refused
  (`install.dev-mode-required`, v1.1).
- Its shallow verify compares against the inventory's hashes.
- It MUST NOT use an id in a first-party scope (`trust.sideload-scope`).

## 13. Public and private modules (user requirement, 11/09)

### 13.1 The rule

- A module is **public** or **private** (`visibility` in `module.toml`, default `public`).
- **Public** modules live in the public repo or registry.
- **Private** modules live in a separate private repo or a local folder.
- Both are installed, verified, enabled, hot-swapped, rolled back and removed the same way (§7).
  No step may branch on visibility except where this section says so.

### 13.2 Nothing private in public

- Nothing about a private module may appear in the public repo: not its id or name in code, docs,
  tests, test vectors, schemas, examples, logs checked in, or crash reports.
- The hub and every first-party module MUST NOT hold a hard-coded module list (§2.2). Behaviour for
  a module comes from its manifest, its catalog entry and facts.
- A private module's id MUST NOT start with a reserved first label (§4.3). Private ids MUST be
  reverse-DNS under a name the user owns.
- **The gate checks it.** `scripts/check.ps1` reads a deny-list of private names held only on the
  PC, outside the repo, and fails if any appears in the tree. The list's contents and its path's
  contents never enter the repo; without the list, the check is skipped with one line.
- A public module MUST NOT `require` a private module, and a public catalog MUST NOT list one.
- Private modules MAY require and call public ones like any module.

### 13.3 What visibility changes

Only these:
- the module comes from a private source (§13.4) and verifies against that source's anchor
  (§13.6);
- shared output redacts it (§13.5);
- MCP leaves it out by default (§9).

### 13.4 Module sources and discovery

Modules are discovered **only** from configured module sources and the manifests in them.
`hideout.toml` lists them:

```toml
[[sources]]
id = "<source id>"                # a-z, 0-9, -; unique
kind = "catalog"                  # catalog | git | folder
visibility = "public"             # public | private
url = "https://…/catalog.json"    # catalog: an https URL; git: a repo URL; folder: absent
path = ""                         # folder: an absolute local path; otherwise absent
anchor = "<trust anchor id>"      # §13.6; absent only for a developer-mode folder
```

- **Kinds:**
  - `catalog`: a signed `catalog.json` at an https URL (the public registry is one).
  - `git`: a private git repo holding a signed `catalog.json` and module archives; credentials come
    from Windows Credential Manager or git's credential helper, never from a manifest, a catalog
    or `hideout.toml`.
  - `folder`: a local folder holding a signed `catalog.json` and archives, or, in developer mode,
    unsigned module folders.
- **Checks on `hideout.toml` (v1.1):** a reader refuses the file, with these ids, when
  - no `public` source is listed (`config.no-public-source`);
  - a `folder` source has a `url`, or a `catalog` or `git` source has a `path` (`config.folder-url`);
  - a source `id` isn't `a-z`, `0-9`, `-`, or two sources share one (`config.source-id`);
  - a source's `anchor` names no `[[trust.anchors]]` entry (`config.unknown-anchor`).
- There MUST be at least one `public` source (the Hideout registry, shipped in the MSI config) and
  MAY be any number of `private` ones.
- A `private` source's modules MUST have `visibility = "private"`; a `public` source's MUST be
  `public` (`trust.visibility`).
- Sources are fetched on the user's action or at the hub's update, never on a timer. A `folder`
  source is noticed by a directory-change notification.
- **Where a source's root is (clarification, #1049).** A source anchored to the Hideout root has
  its `root.json` beside its `catalog.json`: the same URL path with the last segment replaced
  (`https://…/c/catalog.json` → `https://…/c/root.json`, query and fragment dropped), or the same
  folder. Each file's signature is the file's name plus `.minisig`. A source with a user anchor
  (§13.6) has no `root.json`.
- Adding or removing a source is a user action in the Modules page or the CLI. A source is never
  added by a module or an outside tool.
- Removing a source leaves its installed modules installed; their updates stop.

### 13.5 Redaction in shared output

The hub MUST be able to redact or omit a private module's identity in anything shared:
- **Copy diagnostics, crash reports, health signals and telemetry:** by default a private module
  appears as `private-<n>`, a stable per-install alias (an HMAC of the id with a secret kept in
  `hideout.toml`, truncated). Its name, version, paths, method names, fact names, panel titles and
  source URL are left out.
- `[privacy] share_private = "alias" | "omit" | "full"` (default `alias`). `full` is a deliberate
  user choice per copy, never a saved default.
- **Logs** on disk MAY name private modules, since they stay local. Any log excerpt that becomes
  shared output is redacted by the same rule before it leaves the hub.
- The crash breadcrumb (§5.5) and module stderr captures follow the same rule when shared.
- Facts about private modules (`module.<id>.*`) are not sent over MCP or to the tailnet unless
  `[privacy] agents_see_private = true`.

### 13.6 Trust anchors

A trust anchor is a key a source's catalog must verify against. `hideout.toml` lists them:

```toml
[[trust.anchors]]
id = "<anchor id>"
kind = "root"                     # root | user
key = "RWQ..."                    # minisign public key
scope = ["<reverse-dns>.*"]       # user anchors only: the ids it may sign
```

- A source's `anchor` MUST name an anchor listed here (`config.unknown-anchor`, v1.1).
- **`root`:** the Hideout root (§12.2), pinned in `hideout.exe`. It cannot be removed or replaced
  through `hideout.toml`.
- **`user`:** a local or user signing key the user adds (for a private repo or folder). It signs a
  `catalog.json` directly; there is no `root.json` above it.
  - Its `scope` MUST NOT overlap any scope of the Hideout root (`trust.scope-overlap`), so it can
    never sign a first-party id.
  - Adding one is a user action, shown with the key's fingerprint.
- Every rule of §12.4 and §12.5 applies to user-signed catalogs (signature, scope, expiry,
  sequence, `seen` per anchor).
- Providers: a user-signed catalog may fix `provide` only for method areas no root-signed catalog
  assigns (§6.3).
- A module signed by a user anchor counts as "signed by a user key" for §6.6: it may be granted
  `key.events`, `hid.raw` or a `keys` channel with the red warning, never over MCP.

## 14. Naming (pick H, UX20)

- **UI words:** "Modules" (the page and each item) and "panels" (what a module declares). "Cards"
  and "Plugins" are not used in the UI. In code: `module`, `ui.register {panels}`.
- Each module shows a plain function name in the UI (Keyboard, Lights, Scenes, Screens, Art,
  Overlay, Keymap, Firmware, Find my LEDs, Wallpaper Engine, Mic, Batteries, AI).
- **Info screen:** the keyboard screen the PC draws on (it was the "left panel"). In code:
  `info_screen`, `crates/info-screen`. Screens are named by role, never by side.
  - The UI MUST NOT use "Info" alone as a label.
  - The first time it appears (first run and the Screens page), a small board drawing marks the
    Info screen and the Art screen.
- **Wire names stay:** `screens.panel_override`, `screens.panel_release` and the SDK's `panel_*`
  keep working for good. `screens.info_*` aliases arrive in a minor; the SDK gains
  `info_screen_*`, and the old names are deprecated, not removed.
- **Hideout** is the product. **Foxglove** names the keyboard family (`foxglove.*`, Digitalis, the
  models). **Vulpes** is the AI module (Arete today).
- "Style" is for image generation only; it never stands for palette, wallpaper, effect or look.

## 15. Error ids

Readers report a failed check with one of these ids (`error.id` in `--json`, the error's `id()` in
Rust, the exception's `id` in Python). Message text is free; the id is the contract.

| Id | When |
|---|---|
| `manifest.syntax` | not valid TOML, or a key has the wrong type |
| `manifest.unknown-key` | a key the reader doesn't know |
| `manifest.missing-key` | a required key is missing |
| `manifest.id` | the id breaks §4.3 |
| `manifest.family` | unknown family, or it doesn't match a reserved first label |
| `manifest.kind` | unknown kind |
| `manifest.version` | not SemVer as §4.2 |
| `manifest.hub` | `hub` isn't `MAJOR.MINOR`, or isn't served |
| `manifest.api` | an `api` value isn't `MAJOR.MINOR` |
| `manifest.run` | `run` missing, present where it mustn't be, empty, or leaving the folder |
| `manifest.visibility` | unknown visibility, or a private module with a reserved first label |
| `manifest.requires` | bad `requires` grammar, or a public module requiring a private one |
| `manifest.enriches` | an entry isn't `<name>.exe` |
| `manifest.provides` | unknown entry, or content/firmware without its entry |
| `manifest.activation` | a bad `activation.on` or `when` entry |
| `manifest.limits` | (v1.10) a `[limits]` value of the wrong type or out of range |
| `manifest.ui` | a bad `[[ui.panels]]` or `[[ui.windows]]` entry |
| `manifest.mcp` | a bad `[[mcp.tools]]` entry |
| `manifest.mcp-method` | (v1.1) an `[[mcp.tools]]` `method` the module doesn't provide |
| `manifest.window-missing` | (v1.1) `[[ui.windows]]` without the `ui.window` permission |
| `manifest.pin` | `[[pin]]` misplaced, missing, or bad |
| `manifest.edition` | `[edition]` without `content:edition`, or the reverse |
| `manifest.identity` | `[identity]` on a module that may not have it |
| `manifest.actions`, `manifest.settings` | as plugin.toml |
| `manifest.secrets` | `[[secrets]]` without `secrets` |
| `permission.unknown` | unknown permission |
| `permission.argument` | bad argument |
| `permission.never` | a never-list item asked for where it can't be granted |
| `permission.provider` | registering a method the catalog doesn't allow |
| `ui.undeclared` | `ui.register` with a panel id not in `[[ui.panels]]` |
| `ui.limits` | a panel tree over a cap (v1.4: and a tile over the tile caps; v1.5: a stream in a tile) |
| `ui.prop` | (v1.4) a `tone` that isn't a string, a button `opens` that isn't `page`, `page#<node id>` or `settings`, an `action` that isn't an action id, or a button with both; (v1.5) a canvas `width`/`height` or image `aspect`/`width` that isn't a number above 0, an image `scaling` that isn't a string, a text `mono` that isn't a boolean, a `stale_after_ms` that isn't a whole number above 0, or a streamed image without a `label` |
| `ui.action` | (v1.4) a button `action` that names no `[[actions]]` entry of the module |
| `lifecycle.api` | a needed API isn't served |
| `lifecycle.requires` | a required module is missing, too old or off, or a cycle |
| `lifecycle.required-by` | disabling would break an enabled module |
| `lifecycle.declined` | the user declined the permissions |
| `lifecycle.edition` | an edition lists a module that isn't installed (reported) |
| `lifecycle.no-hub` | a step that needs the hub, with the hub not running |
| `install.dev-mode-required` | (v1.1) an install outside any catalog without developer mode |
| `config.no-public-source` | (v1.1) `hideout.toml` lists no `public` source |
| `config.folder-url` | (v1.1) a `folder` source with a `url`, or another kind with a `path` |
| `config.source-id` | (v1.1) a source id breaks §13.4 or is used twice |
| `config.unknown-anchor` | (v1.1) a source's `anchor` names no `[[trust.anchors]]` entry |
| `config.windows` | (v1.6) a `[windows]` entry breaks §2.5's shape; the hub drops that page's entry |
| `trust.signature` | a signature doesn't verify |
| `trust.root-expired`, `trust.root-rollback` | the root expired or is older than `seen` |
| `trust.scope` | an entry is outside its family's or anchor's scope |
| `trust.scope-overlap` | two scopes cover the same id (families, or a user anchor and the root) |
| `trust.key-reuse` | one key in two families |
| `trust.epoch` | a catalog's epoch isn't its root's |
| `trust.expired` | a catalog expired |
| `trust.rollback` | a catalog's sequence is below `seen` |
| `trust.unknown-archive` | the archive isn't in any accepted catalog |
| `trust.yanked`, `trust.revoked` | the version was withdrawn |
| `trust.files` | the folder doesn't hold exactly the signed files |
| `trust.mismatch` | manifest id, version, kind or visibility differs from the entry |
| `trust.visibility` | a module's visibility doesn't match its source's |
| `trust.sideload-scope` | a sideloaded module uses a first-party id |
| `rpc.duplicate-request-id` | (v1.2) a request id already in flight on the connection: error -32600, id null (§8.1) |
| `rpc.frame-too-large` | (v1.2) a line over 256 KiB: error -32600, id null, then the channel closes (§8.1) |
| `channel.pipe` | (v1.4, #977) a provider answered `channel.create` with no pipe or one that is not `\\.\pipe\hideout-ch-<rest>`: the consumer gets error 6 with this `data.id` (§8.4) |
| `rpc.*` | JSON-RPC transcripts use the numeric codes of §8.1; the two ids above travel in the error's `data.id` |

## 16. Test vectors

`tests/hideout-vectors/` (#906) holds valid and invalid `module.toml` files, legacy `plugin.toml`
and `kit.toml` files, permission strings, roots, catalogs, `hideout.toml` source and anchor
tables, panel trees (under and over the caps), and JSON-RPC transcripts for every message in §8.8
(v1.3: `transcripts/ui/badge.json` and `transcripts/ui/visibility.json` for §5.9; v1.4: `tone`,
`opens`, `action` and tile-cap panel vectors, `kind = "file"` and `apps.seen` manifests, a stdio
module with panels, tile steps in `visibility.json`, `transcripts/supervisor/outside-app-missing.json`,
and `facts/supervisor-facts.json`; #977: `channels/channel-wire.json`, the channel wire as hex
streams with the end each reaches and the pipe names a consumer may connect to, and
`transcripts/stream/channel-create-bad-pipe.json`; v1.5: the stream props as panel vectors, valid
and invalid one each, a stream in a tile, a whole and a split multibyte text frame in
`frames/panel-frames.json`, `transcripts/stream/text-restore-on-visible.json` and
`transcripts/stream/reopen-then-stopped.json`; v1.7: `facts/app-facts.json`,
`manifests/invalid/when-apps-running-with-exe.toml` and `transcripts/ui/asset-put-refused.json`;
v1.8: `panels/valid/at-cap-*.json` and `panels/invalid/over-cap-*.json`, each page cap at the cap
and one over, and `transcripts/supervisor/backoff-resets-after-quiet.json`; v1.11:
`trust/hub-release.json`, a catalog entry's `min_hub_release` and the page Update Hideout opens). The vectors are
edited by hand, each in the PR that changes its result: the generator #914 used was never
committed (`**/tools/*` is git-ignored) and no copy survives. A future generator lives at a tracked
path (`scripts/`).
A panel vector's index entry may carry `context.actions`, the module's `[[actions]]` ids. The
spec version is kept in step by a drift test: this file's status line, `index.json`'s `contract`,
the vectors README and the schema's `spec` (`SPEC_VERSION` in the Rust crate) must agree.
`index.json` gives each file's expected result: `valid` (with the normalized fields) or `invalid`
(with the §15 id). The Rust crate and Vulpes's models MUST pass every vector. A spec change that
changes a result changes the vector in the same PR. Vectors MUST use placeholder ids only and
never name a private module (§13.2).
