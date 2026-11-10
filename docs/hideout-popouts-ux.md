# Hideout pop-out windows (UX plan)

Issue #998 (H2.11), UX gate step. Any page can leave the hub's page area for a window of its own
and come back. Contract fields: [hideout-contract.md](hideout-contract.md) §2.5 (window rules),
§5.6 (view state), §5.7, §5.9 (tabs, `ui.visibility`), §7.6 (hot swap), §7.9 (hub restart).
Earlier plans this one builds on, with the same words: [hideout-tabs-ux.md](hideout-tabs-ux.md)
(the placeholder tab, Dock back), [hideout-supervisor-ux.md](hideout-supervisor-ux.md) (crash bar,
stopped card), [hideout-streams-ux.md](hideout-streams-ux.md) (streams at window size).
Clean-room: written from the contract, the design doc and public docs; no code under `app/` was
read or used.

Mockups (Python + Pillow, no window opened,
[`design/mockups/hideout_popouts_mockups.py`](design/mockups/hideout_popouts_mockups.py), the tab
and supervisor mockups' helpers and `hideout-shell` DARK tokens; the script fails under 4.5:1,
lowest 5.4:1):
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h211\ux-h211-proposed-windows.png`: the hub with
  placeholder tabs, two pop-outs (one kept on top, its More menu open), the taskbar group
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h211\ux-h211-proposed-gestures.png`: pop out from
  the tab menu, drag a tab out, drag a window back onto the strip, the keyboard
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h211\ux-h211-proposed-states.png`: module states in
  a pop-out, Hideout in the tray with a pop-out open, placement per screen and the fallback

## Research

- **Reuse:** egui viewports (the hub already opens a second one for a settings window); the panel
  host draws a panel by `PanelKey` and keeps view state by node id (§5.6), so a pop-out is "the same
  panel in another viewport", and a hot swap keeps its place for free; the tab strip's popped-out
  placeholder, the dirty ask and the restore list (tabs plan); the crash bar, stopped card and
  `state_words` (supervisor plan); streams rescale with their box (streams plan).
- **Emulate:**
  - **VS Code floating editor windows:** "Move into New Window", "Move back into Main Window",
    an always-on-top pin in the window's title area, the window closing back into the main one.
  - **Browsers and Windows Terminal:** drag a tab out to tear it off, drag it onto a strip to put
    it back, with an insertion mark; Esc cancels a drag.
  - **Outlook and Teams pop-outs:** a small ↗ button on the thing itself, one click.
  - **Windows window placement:** windows come back where they were per display layout, never off
    screen; sizes in device-independent units (per-monitor DPI v2); snapped and maximised states
    come back.
  - **Windows taskbar grouping by AppUserModelID:** every Hideout window under one button, a
    thumbnail per window titled "Hideout · <page>".
- **Not taken:** the same page in two places at once (VS Code allows it; our view state is one
  per node id), several tabs in one pop-out (tab groups, out of scope in the tabs plan), pop-outs
  owned by the main window (they would minimise and hide with it), a module asking to pop itself
  out (the hub owns windows, §5.9).

## Scope

In: the hub's own windows for any `page` panel, built-in pages (Modules, Settings) included
(`docs/scope.md`: one place to see the keyboard, lights and screens; a page on a second screen is
the same place). No module code: it is the same declared panel, so it works for every module and
language. Out: module windows (editors and wizards, §5.7; they follow the same title, AUMID and
placement rules but are the module's), tiles, the tray panel, the overlay, tab groups, split views,
click-through windows.

## Model

- A page is in exactly one place: a tab in the hub's page area, or a pop-out. Popping out leaves
  a **placeholder tab** (tabs plan: dimmed, ↗, same place, same Ctrl+number).
- A pop-out is an **unowned top-level window** of the hub process: it doesn't minimise, hide or
  close with the hub's main window.
- Any number of pop-outs, one per page. New ones cascade 32 px from the last if they would cover
  it exactly.
- The hub saves, per page: popped out or not, placement per screen layout, Keep on top. These live
  in the hub's own files (§2.3, §5.9), never in a module's.

## Popping out

| Way | What happens |
|---|---|
| Right-click a tab > **Pop out** | Window opens next to Hideout on the same screen, offset 32 px, at the page area's size (at least 480 × 360). Focus moves into it. |
| **Ctrl+Shift+O** on the active tab | Same. |
| **Pop out** button (↗) at the strip's right end, for the active tab | Same. (Question 3.) |
| Ctrl+K > "Pop out <page>" | Same; works for a page that isn't open as a tab yet. |
| **Drag a tab** off the strip and out of the window | Past 24 px from the strip a ghost card follows the pointer: "Let go to open in its own window" / "Esc cancels". Letting go outside Hideout opens the window there, its title row under the pointer. Letting go on the strip only reorders; on the page area or Esc cancels. |
| The page was saved as popped out | It opens in its window at its saved place (Restoring, below). |

When the active tab pops out, the hub activates the tab to its right, else the left one (the
tabs plan's close rule). If no other tab is open, the page area shows "Nothing open" with the line
"Chat is in its own window." and **Show it** · Dock back.

## The window

- **Title:** `Hideout · <page title>` (§2.5), following the panel's `title`; a new registration
  that renames the page renames the window.
- **Icon:** Hideout's icon for the window, Alt+Tab and the taskbar, so every window reads as
  Hideout. The page's icon shows only in the window's own title row, before the title.
- **Taskbar:** Hideout's AUMID, one button for all Hideout windows; hovering shows a thumbnail per
  window. With "Never combine" the user gets one button per window, each titled as above.
- **Title row** (one row, 36 px; mockup "windows"): page icon, title, **Keep on top** (pin, a
  toggle), **Dock back** (icon + label), **More** (…), then Windows' minimise, maximise, close.
  More holds: Dock back (Ctrl+Shift+O), Keep on top, Show Hideout, Close page.
- **Size:** at least 360 × 240. Resizing re-lays out the page and rescales streams, never restarts
  them (streams plan).
- **Keep on top:** off by default, remembered per page. On: the pin fills orange and the window
  stays above other windows (not above an exclusive full-screen game, which Windows doesn't allow).
  It never takes focus by itself.
- Snap layouts, Win+arrows and the window menu (Alt+Space) work as on any window. Hideout adds
  **Keep on top** and **Dock back** to the window menu.

## Docking back

| Way | Where it lands | Hideout's main window |
|---|---|---|
| **Dock back** button, More > Dock back, Ctrl+Shift+O, window menu > Dock back | Its placeholder's place, as the active tab | Shown and brought forward (out of the tray if needed); focus on the page |
| **Drag the window** by its title row onto the strip | Where the orange insertion bar shows; the window closes on letting go | Already in view (you dropped on it) |
| **Close** (× or Ctrl+W or Alt+F4) | Its placeholder's place, as a normal tab, not activated | Left as it is: in the tray stays in the tray, behind stays behind (Question 2) |
| More > **Close page** | The tab closes too (tabs plan's close) | Left as it is |

- Closing a pop-out loses nothing: the page goes back to its tab with its view state. So **closing
  never asks**, even with unsaved work. Only **Close page** asks, with the tabs plan's dialog
  ("Close Chat?" / "Your changes in Chat aren't saved." / **Keep open** · Close anyway).
- Dropping a dragged window on another pop-out does nothing (no tab groups); the pointer shows
  "no drop" and the window stays where it was let go.

## The hub while a page is out

- **Placeholder tab:** dimmed, ↗ after the title, tooltip "Chat · In its own window". Clicking it
  (or Ctrl+number, or Enter) brings the window forward; it never shows the page in the hub too.
- **Sidebar entry:** normal, with a muted ↗ at its right; clicking it brings the window forward.
  The entry's badge (dot, count, stopped mark) stays as it is for any page.
- **Badges** follow the page: the placeholder and the sidebar entry show the page's badge as
  usual; the pop-out shows nothing extra in its title row (the page itself shows what's new).
- **Hideout in the tray** (main window closed to the tray): pop-outs stay open and live. The
  taskbar button stands for the pop-outs only. The tray icon opens Hideout's main window, not the
  pop-outs. **Quit Hideout** in the tray menu closes every window, after one ask listing unsaved
  pages (tabs plan).
- **Minimising Hideout** leaves pop-outs as they are; minimising a pop-out leaves Hideout as it is.

## Module states in a pop-out

The pop-out shows exactly what the page would show, in the same words (supervisor plan, §7.8):

| Module state | In the pop-out | Placeholder tab and sidebar |
|---|---|---|
| Starting | Page from the cache, header "Starting…", controls off until ready | orange ring |
| Updating (hot swap, §7.6) | One header line "Updating… Your place on this page is kept." | orange ring |
| Restarting | The amber crash bar: "Chat stopped unexpectedly. Restarting in 4 s (2 of 5)." / "Your place on this page is kept. · Open log" | orange ring |
| Stopped (crash-stopped, needs-hub, …) | The stopped card with its ladder, the last page greyed below | red warning mark |
| Off | "Off" + **Turn on** | muted |
| Removed | The window closes with its tab. Hideout's status strip: "Art was removed; its window was closed." | tab gone |

- **View state through a hot swap or restart:** scroll position, open sections, the selected
  tab, typed text, a stream's follow state, all kept by node id (§5.6). The window doesn't close,
  move, resize or take focus. This is H2.11's lab acceptance.
- The window's title follows the new registration's `title`; its placement never changes.
- A crash bar's countdown repaints only while the pop-out is visible (supervisor plan).

## Visibility to the module

From §5.9, with pop-outs spelt out (spec gap 1):
- pop-out on screen, not minimised: `visible`, also while Hideout's main window is in the tray;
- pop-out minimised: `hidden` (streams closed, last frame kept);
- popped out: the module gets nothing new (it was `visible` as the active tab or `hidden` as a
  background tab; the pop-out is `visible`);
- docked back: `visible` if it lands as the active tab of a shown hub, else `hidden`. Never
  `closed`, because closing a pop-out doesn't close the page;
- Close page: `closed`.

## Placement and restoring

- **Saved per page and per screen layout.** A screen layout is the set of connected screens, each
  with its position, size and scaling. Up to 4 layouts per page are kept; the oldest goes.
- **Saved:** position and size in device-independent units, the screen it is on, maximised or
  snapped, Keep on top. Saved when the window stops moving or resizing and when it closes, never
  on a timer.
- **Restoring:** the saved place for the current layout if there is one. Else, the place saved for
  that same screen in another layout, if that screen is connected. Else Hideout's own screen,
  offset 32 px, same size in screen-independent units, clamped fully on screen. At least the title
  row is always on a screen.
- **Scaling:** moving a window between a 100% and a 150% screen keeps its size to the eye and
  re-lays out crisply (per-monitor DPI v2), without restarting streams.
- **Screen unplugged while open:** Windows moves the window; Hideout saves the new place for the
  new layout and keeps the old one for when the screen is back.
- **When pop-outs come back:**
  - Hub restart after a crash or GPU reset (§7.9): every pop-out comes back at its place at once,
    from the cache with "Starting…" (§2.4).
  - Hideout opened by the user (Start menu, tray): every saved pop-out comes back with it.
  - Hideout starting in the tray at sign-in: pop-outs wait and come back when the user first opens
    Hideout (Question 1). Pages are restored as placeholder tabs meanwhile.

## Keyboard and screen reader

- **Keys:** Ctrl+Shift+O pops out the active tab, and in a pop-out docks it back. Ctrl+W in a
  pop-out closes the window (back to its tab). Ctrl+K works in a pop-out: picking a page shows it
  in Hideout (or brings its window forward). F6 moves between the title row and the page. Alt+Space
  opens the window menu with Keep on top and Dock back. Esc cancels a tab drag. Ctrl+Tab and
  Ctrl+numbers do nothing in a pop-out (one page).
- **Windows to cycle:** Alt+Tab and Win+number (the taskbar group) already reach each window; no
  own shortcut.
- **Focus:** popping out by menu, key or button moves focus to the page in the new window, on the
  element that had it. Dock back puts focus on that element in Hideout. Close leaves focus to
  Windows.
- **AccessKit:** the window's name is "Hideout · Chat"; the title row is a toolbar; Keep on top is a
  toggle button ("Keep on top, on"); Dock back a button; the placeholder tab's name is "Chat, in
  its own window" (tabs plan).
- **Announced once, politely:** "Chat opened in its own window", "Chat docked, tab 3 of 4". The
  crash bar and the stopped card announce as on a page.
- Colour is never the only signal: the pin has a filled state and an accessible state; the ↗ mark
  has a name. All drawn labels ≥ 4.5:1 (script).

## Wording

| Where | Text |
|---|---|
| Tab menu, palette | "Pop out" (palette: "Pop out <page>") |
| Strip button tooltip | "Pop out (Ctrl+Shift+O)" |
| Drag ghost | "Let go to open in its own window" / "Esc cancels" |
| Window title | "Hideout · <page>" |
| Title row | Keep on top (tooltip "Keep on top" / "Keep on top: on"), "Dock back", More |
| More menu | Dock back, Keep on top, Show Hideout, Close page |
| Placeholder tooltip | "<page> · In its own window" |
| Empty page area | "Nothing open" / "<page> is in its own window." / Show it · Dock back |
| Removed module | "<Module> was removed; its window was closed." |
| Close page with unsaved work | tabs plan's "Close <page>?" dialog |

Same verbs as the tabs plan: "Pop out" and "Dock back", never "detach", "undock", "float" or
"tear off". No internal ids (no panel ids, viewport ids, AUMID strings) anywhere in the UI.

## Acceptance (for app-engineer)

1. Any page, built-in or module, pops out by tab menu, Ctrl+Shift+O, the strip button, the palette
   and a drag; and docks back by Dock back, Ctrl+Shift+O, a drag onto the strip and closing.
2. The window's title is "Hideout · <page>", it groups under Hideout's taskbar button, and it is
   not owned by the main window (hiding Hideout to the tray leaves it up).
3. Placement is restored per screen layout, in device-independent units, never off screen, with
   the fallback above. A lab case covers a missing screen.
4. Lab: a popped-out page keeps scroll, an open section and typed text through a module hot swap
   (overlap and handover) and a crash restart; the window doesn't move or take focus.
5. Closing a pop-out never asks; Close page asks only with a dirty badge.
6. `ui.visibility` follows the list above (lab case with a test module recording its messages).
7. Footprint: H2.10 records one pop-out's cost (RAM, wakeups) at idle.

## Spec gaps

1. **§5.9 `closed` and the dirty ask.** The contract says `closed` is sent when "the user closed the
   tab or its pop-out", and that closing a pop-out asks first with a `dirty` badge. In this plan,
   closing a pop-out docks the page back, so it is neither `closed` nor asks; only Close page is.
   §5.9 and §2.5 ("ask on close only if work would be lost") should say so.
2. **§5.9 `hidden` for the hub in the tray.** "The hub hidden in the tray" makes panels `hidden`;
   it should be "panels in the hub's main window". A pop-out stays `visible` while the main window
   is in the tray.
3. **§2.5 restore timing.** "Reopen at their places after a hub restart" doesn't say whether a
   normal start at sign-in in the tray reopens them (Question 1).
4. **§2.3 / §5.9 saved state.** Placement per page and per screen layout (how many layouts kept,
   what identifies a screen across reconnects) isn't in the hub's file list; it should be named in
   `hideout.toml`'s section or its own hub-owned file.
5. **Window ownership.** §2.5 doesn't say pop-outs are unowned top-level windows; owned windows
   would hide with the main window.
6. **Drag between windows.** Docking by dragging a pop-out onto the strip needs the hub to see a
   window move over another of its viewports; the contract has nothing on cross-viewport drag, and
   egui viewports may not report it. If it can't be done, Dock back and close remain, and the drag
   ships later.
7. **Module windows.** §2.5's shared rule set should say whether module windows also get Keep on
   top and the same placement memory keyed per screen layout.

## Open questions for the user (via the Hub)

1. **Pop-outs at sign-in** when Hideout starts in the tray: **come back when you first open
   Hideout** (recommended; quiet start, nothing appears over a game) / reopen at sign-in.
   Image: `C:\Users\fuchs\splitkeeb\worktrees\_review\h211\ux-h211-proposed-states.png`.
2. **Closing a pop-out (×):** **page goes back to its tab quietly, Hideout stays where it is**
   (recommended; Dock back is the "show me in Hideout" way) / goes back and shows Hideout with that
   tab. Image: `C:\Users\fuchs\splitkeeb\worktrees\_review\h211\ux-h211-proposed-windows.png`.
3. **A Pop out button (↗) at the tab strip's right end:** **yes, for the active tab** (recommended;
   one click, like Outlook and Teams) / no, tab menu, drag, keys and palette only.
   Image: `C:\Users\fuchs\splitkeeb\worktrees\_review\h211\ux-h211-proposed-gestures.png`.
