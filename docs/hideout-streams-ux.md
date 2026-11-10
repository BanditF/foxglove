# Hideout live streams in panels: what the user sees (UX plan)

Issue #976 (H2.9), UX gate step. Three panel nodes take a `stream` (contract §5.4): `canvas`
(colours for its shapes), `image` (frames) and `text` (appended text). The hub's panel host reads
each one as a channel end (§8.4), only while the panel is visible, and draws at most 30 frames a
second. This plan is one pattern per stream kind, for any module: its states, what a frozen frame
looks like, sizing and scaling, the text stream's follow, copy and trim, reduced motion, the screen
reader and the words. Clean-room: written from [hideout-contract.md](hideout-contract.md) v1.4
(§5.2–5.5, §5.9, §7.8, §8.4 including the channel wire written down in #977/#988),
[hideout-tabs-ux.md](hideout-tabs-ux.md), [hideout-supervisor-ux.md](hideout-supervisor-ux.md),
[hideout-plugin-modules-ux.md](hideout-plugin-modules-ux.md) and public docs; no code under `app/`
was read.

Mockups (Python + Pillow, [`design/mockups/hideout_streams_mockups.py`](design/mockups/hideout_streams_mockups.py),
the tab and supervisor mockups' helpers and `hideout-shell` DARK tokens; one `stream_box()` draws
every canvas and image state, one `text_stream()` every text state; the script fails under 4.5:1,
lowest 5.8:1; no window opens):
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h29\ux-h29-proposed-states.png`: every state of a
  canvas/image stream (connecting, live, back on screen, paused by you, no new frames, stopped
  updating, module restarting, couldn't be shown) and of a text stream (following, scrolled up,
  copy, module restarting)
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h29\ux-h29-proposed-page.png`: the lab sample, a
  module page with a room map (canvas), a screen preview (image) and an activity log (text), live
  and under the amber crash bar
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h29\ux-h29-proposed-sizing.png`: canvas and image
  scaling, no layout jump on a new frame size, reduced motion, the pause control

## Research

- **Reuse:** §5.4's rules (visible only, 30 fps, the hub knows no domain), `ui.visibility`'s
  `visible`/`hidden`/`closed` (§5.9, the tabs plan's definition of hidden), the supervisor's amber
  crash bar and stopped card (§7.8), the stale view's "Last seen <age>" and greyed body
  (plugin-modules plan), the panel guard's "couldn't be shown" wording (§5.5), UX17's "Jump to
  latest" chip and per-message Copy, developer mode (§12.6) for numbers a user never needs.
- **Emulate:**
  - **OBS preview / Stream Deck key images:** live pixels with no "LIVE" badge on a preview; the
    host scales frames into a fixed slot and never reflows around them.
  - **Browsers' background-tab throttling and `requestAnimationFrame`:** nothing is drawn or
    decoded while hidden; on return the last frame shows until the next one, with no spinner
    flash.
  - **Video players and GIF controls (WCAG 2.2.2 Pause, Stop, Hide):** moving content that starts
    by itself gets a pause control; a paused frame is shown at full colour with "Paused".
  - **Terminal scrollback and VS Code's output/chat panes:** follow the end while at the bottom,
    stop when the user scrolls up, a chip to jump back; old lines drop from the top past a cap
    with a line saying so; Copy copies the whole pane.
  - **CSS `object-fit: contain` and `image-rendering: pixelated`:** fit inside, keep the shape,
    letterbox; whole-number steps without blur for pixel art.
  - **Screen readers on live media (ARIA `aria-live="off"` on video/canvas, VS Code chat's
    "response received" signal):** frames are never read; one quiet cue when text stops arriving.
- **Not taken:** an FPS counter or "Live" dot in normal use (noise; the motion is the sign), a
  "Paused" label on every return from a background tab (the user paused nothing), a spinner on
  every tab switch, stretching frames to fill, per-module stream chrome, announcing text as it
  streams.

## Scope

In: how the hub draws stream-bound `canvas`, `image` and `text` nodes on pages and in pop-outs,
their states, sizing, the pause and copy controls, reduced motion and the screen reader
(`docs/scope.md`: one place to see the keyboard, lights and screens and know they work; §5.4: a
page that shows what your devices are doing stays in the hub). Out: what any module streams, the
channel grant flow itself (H4.3, no user-facing step), module windows (§5.7), streams in tray tiles
(see spec gap 1), a `paint` node, the Chat page's own layout (H5.5 builds on the text pattern here).

## Shared rules (all three kinds)

- **The hub owns the frame, the module owns the content.** Every state mark, caption, chip and
  control below is hub-drawn from the stream's channel state and `module.<id>.state`; the module's
  frames never draw them, and nothing here names a module or a domain.
- **Visible only.** The channel is open only while the panel is `visible` (§5.9). Hidden means
  not on screen, so the user never looks at a "paused because hidden" stream; what matters is the
  return (below).
- **Latest frame wins.** Frames are drawn at the next repaint, at most 30 a second; frames in
  between are skipped, never queued, so a slow PC shows the newest colours, not a backlog.
- **Root cause once.** When the module is restarting, stopped, updating or off, the page's crash
  bar or card (§7.8) says why. A stream node only dims and says "Last seen"; it never has its own
  error words for a module state.
- **No layout jump.** A stream node's box is set before the first frame and never resizes because
  a frame arrived or changed size.

## States: canvas and image (one pattern)

`ux-h29-proposed-states.png`, top two rows. The box keeps its size in every state.

| State | When | What the box shows | Ways out |
|---|---|---|---|
| Connecting… | channel opening, no frame yet | empty box; after **1 s** a small ring + "Connecting…" (never before 1 s, so a fast open shows nothing) | none needed |
| Live | frames arriving | the frames, full colour, no mark (a Live dot with reduced motion on) | Pause (hover/focus) |
| Back on screen | panel turned `visible` again; channel reopening | the **last frame, unchanged, no mark** for up to **2 s**; then the next frame replaces it | none needed |
| Paused by you | the user pressed Pause | last frame at **full colour**, "Paused" chip top-left, Play button bottom-right; the channel is closed as if hidden | Play, Space |
| No new frames | open channel, no frame for longer than the node's expected gap (spec gap 3); also "back on screen" past 2 s | last frame **dimmed**, chip "Last seen <n> s ago" | none (it recovers by itself) |
| Stopped updating | the provider closed the channel while the module runs and the panel is visible; the hub's 3 reopens (1, 2, 4 s) failed | last frame dimmed, chip "Stopped updating" + **Try again** | Try again (one reopen) |
| Module restarting / updating / stopped / off | `module.<id>.state` ≠ `running` | last frame dimmed, chip "Last seen <n> s ago"; the crash bar or stopped card above says why | the bar's or card's |
| Couldn't be shown | the stream was refused or closed with `ui.limits` (too many streams, frame too big, over 60 a second), or the grant failed | no frame; warning mark, "This preview couldn't be shown." + **Open log** | Open log |

Rules:
- **What a frozen frame looks like.** Full colour when the user chose it (Paused) or when it is
  only waiting to be replaced (back on screen, under 2 s). **Dimmed** (the stale view's greyed
  body: frame at about 40% over the surface) only when it is out of date for a reason the user
  should know: no new frames, stopped updating, the module not running. Dimmed always comes with
  words ("Last seen …" or "Stopped updating"), so dimming never stands alone.
- **No "Paused" label for hidden.** The word is kept for the user's own Pause, which has a Play
  button next to it.
- **Updating keeps full colour.** During a hot swap (`updating`, "Your place on this page is
  kept") the last frame stays undimmed for up to 2 s like "back on screen"; past that it dims.
- **Dropped frames are silent.** A frame whose length doesn't match its header (§8.4) is dropped
  and counted in the log, never drawn as an error. Developer mode (§12.6) shows frames a second
  and dropped frames in the node's tooltip; normal use never shows numbers.
- **Clicks on canvas shapes** (`ui.event` with the shape id) work while live, paused and back on
  screen; while dimmed they are greyed like other inputs of a module that isn't running (UX3's
  queue applies to actions only).

## Sizing and scaling

`ux-h29-proposed-sizing.png`.

- **Canvas:** shapes are in the canvas's own units; the canvas scales uniformly to the node's
  width (vectors stay sharp at any scale), height = width × the canvas's aspect, capped at 60% of
  the page's visible height (then it fits the height and centres). It stops shrinking at 50%; below
  that the page scrolls sideways rather than drawing unreadable shapes. Shape hit areas grow to at
  least 24 × 24 px (WCAG 2.5.8) without changing what is drawn. Shape text is drawn by the hub at
  no less than 11 px or left out, never squeezed.
- **Image:** the box comes from the node's declared aspect (spec gap 2); frames are fitted inside
  it (contain), centred, and the rest is the surface colour (letterbox). Never stretched, never
  cropped. A frame of another size letterboxes inside the same box; the page does not move.
- **Pixel or smooth:** `pixel` scales in whole-number steps with no blur (1-bit screen previews,
  pixel art); `smooth` filters (photos, art previews). The node chooses (spec gap 2); the default
  is `smooth`.
- **Display scaling:** frame pixels are layout pixels at 100%; at 150% the hub scales them with
  the same pixel/smooth rule. A module never needs to know the user's display scaling.
- **Pop-outs** follow the same rules at the pop-out's size; resizing the window rescales, never
  restarts the stream.

## The text stream

`ux-h29-proposed-states.png`, bottom row; `ux-h29-proposed-page.png`, "Activity".

- **Append only.** Stream frames append (§8.4 kind 3). Replacing or clearing is state, so it is a
  `ui.patch` of the node's text (§5.3); the stream continues after it (spec gap 4).
- **Follow (scroll lock).** While the view is at the bottom, new text keeps the end in view. When
  the user scrolls up, following stops and the view stays put; a **Jump to latest** chip appears at
  the bottom (UX17's words). Clicking it, or scrolling back to the bottom, follows again. The
  caret mark at the end shows only while text is arriving.
- **Copy.** A **Copy** button on hover or keyboard focus (top-right) copies all text in the node;
  it reads "Copied" for 2 s. Mouse selection copies a part (Ctrl+C). Ctrl+A inside the focused node
  selects that node only. Copying while text arrives copies what is there at that moment.
- **Wrapping.** Soft wrap at the node's width; long unbroken runs break anywhere. The node's
  `mono` choice (log, code) or proportional text (prose) comes from the node (spec gap 4).
- **Trim.** Past the node's text cap the oldest lines drop from the top and the first line reads
  "Earlier text was trimmed." Copy copies what is kept (spec gap 5).
- **Hidden and back.** Text that arrives while the panel is hidden must not be lost: on return the
  node shows everything (spec gap 6). Until the module has caught up, the text shown stays at full
  colour (it is not stale, only short).
- **States.** Connecting shows nothing (an empty log is normal). Module not running: the text
  stays, muted, with "Last seen <n> s ago"; nothing on screen is removed. Stopped updating and
  Couldn't be shown use the canvas/image chip and words. No "No new frames" state: silence is
  normal for text.

## Reduced motion

- With Windows "Animation effects" off, canvas and image streams draw at most **2 frames a
  second**: still the real colours and pictures, as a slow slide show, with no cross-fades. The
  hub may also read less often from the channel; the module is not told.
- Text follows by jumping to the end, never smooth scrolling; the caret mark does not blink.
- The Pause control is there in every mode, so motion can always be stopped (WCAG 2.2.2).
- At 2 frames a second the motion no longer says "live", so a live canvas or image stream shows
  a small **Live** dot in its top corner (the `live` tone, not colour alone: dot + the word). It
  goes away when the stream pauses, goes stale or ends. Without reduced motion there is no mark.

## Keyboard and screen reader

- **Frames are never announced.** Canvas and image stream nodes are not live regions. A canvas is
  a group named by its label ("Room map, live preview"); its clickable shapes are buttons named by
  their shape text, in declared order. An image stream is an image named by its label; frames
  never change the name.
- **State words are the accessible description:** "live preview", "paused", "not updating, last
  seen 12 seconds ago", "couldn't be shown". A change of description is not announced; the crash
  bar's one announcement (supervisor plan) covers module states.
- **Pause:** Tab reaches the node; Space or Enter toggles Pause/Play; the button's name is "Pause
  Room map" / "Play Room map".
- **Text stream:** not announced while text arrives. After a run of text ends (about 1.5 s with no
  new text) the hub makes **one** polite announcement, "New text in <label>", at most once every
  10 s and not while focus is inside the node. Focusing the node reads its text; the Copy and Jump
  to latest buttons are in the tab order when shown.

## Wording

| Where | Text |
|---|---|
| Connecting (after 1 s) | "Connecting…" |
| Stale chip | "Last seen <n> s ago" / "Last seen <n> min ago" (the stale view's words) |
| Ended chip | "Stopped updating" · Try again |
| Refused | "This preview couldn't be shown." · Open log |
| Live dot (reduced motion only) | "Live" |
| Paused | "Paused" · Play (button names "Pause <label>", "Play <label>") |
| Text | "Jump to latest" · "Copy" / "Copied" · "Earlier text was trimmed." |
| Screen reader | "<label>, live preview" / "paused" / "not updating, last seen <n> seconds ago" / "couldn't be shown" · "New text in <label>" |

Never shown: channel, pipe, nonce, frame, seq, fps (outside developer mode), `ui.limits`,
`channel.pipe`, error 6, byte counts.

## Hand-off to app-engineer (H2.9)

Acceptance, in the lab hub with the sample room map (canvas, image and text streams on one page):
- Live: 20–30 frames a second drawn; frames over 30 skipped, not queued; the router stays idle.
- Minimise the window or switch tabs: the channels close (CPU trace); return: the last frame shows
  undimmed with no "Connecting…" when the reopen is under 1 s.
- Kill the module: the amber crash bar once; each stream box dims with "Last seen"; on restart the
  streams reopen without a click.
- The provider closes a channel: three reopens, then "Stopped updating" + Try again.
- Over-cap stream (a 61 fps or 5 MiB frame sample): "This preview couldn't be shown." + Open log;
  the page's other streams keep running.
- A frame of a new size letterboxes; the page does not move (screenshot diff of the page around the
  box).
- Pause: closes the channel (CPU trace), full-colour frame with "Paused"; Play reopens.
- Animation effects off: at most 2 frames a second drawn.
- Text: follow, scroll-up stop, Jump to latest, Copy all; no string from "Never shown" in any drawn
  text (a string test over the stream node's formatter); no screen reader event per frame
  (accessibility tree event count stays flat while streaming).

## Spec gaps (for the contract owner)

1. **Streams in tiles.** §5.5's tile caps don't say whether a tile body may carry a `stream`.
   Proposal: no in v1 (`ui.limits`); the tray is a glance, and patches cover a countdown.
2. **Box size and scaling props.** §5.2 gives `canvas` and `image` no declared size or aspect,
   so the box can't be laid out before the first frame. Proposal: `canvas` declares its unit size
   (`width`, `height`); `image` declares `aspect` (and optionally `width`); `image` takes
   `scaling: "smooth" | "pixel"` (default `smooth`). Missing aspect: the first frame sets it once.
3. **"No new frames" needs an expected gap.** A channel has no keep-alive, so the hub can't tell a
   still scene from a stalled stream. Proposal: optional `stale_after_ms` on a stream node; without
   it the hub never shows "No new frames".
4. **Text stream and patches; `markdown` streams.** Say that a `ui.patch` of a `text` node's text
   replaces what the stream appended, and that the stream keeps appending after it. §5.4 lists only
   `text`, while the design streams `markdown` replies; say whether `markdown` takes a `stream`,
   and add a `mono` prop to `text`.
5. **Text cap for streamed text.** §5.5's 16 KiB per node is too small for a long reply or log.
   Proposal: a separate cap for stream-appended text (for example 1 MiB per node), oldest whole
   lines dropped first.
6. **Text that arrives while hidden.** The hub closes the stream when the panel is hidden, so
   appended text is lost unless someone restores it. Proposal: on `ui.visibility visible` the
   module patches the node's full text, then streams appends again. Also: a text frame MUST end on
   a character boundary; the hub draws invalid UTF-8 as U+FFFD.
7. **Reopen after the provider closes.** §5.4 doesn't say whether the hub reopens a stream the
   provider closed while the panel stays visible. Proposal: three reopens (1, 2, 4 s), then
   "Stopped updating"; a patch that removes the `stream` prop closes the channel cleanly with no
   state shown.
8. **Visibility inside a page.** `visible` is per panel. A stream node scrolled out of view, or
   a window covered by another, still streams. Proposal: allowed in v1 (the hub may skip drawing
   it); a node-level rule can come later.
9. **Image alt.** A streamed `image` needs a `label` for the screen reader; say it is required
   when `stream` is set.

## Decisions (11/10, Hub, designer's picks)

1. Frozen frame when out of date: **dimmed + "Last seen" chip**.
2. Reduced motion: **2 frames a second**.
3. Live mark: **none** in normal use; with reduced motion on, a small **Live** dot (Hub's tweak:
   at 2 fps the motion no longer signals live). See Reduced motion.
