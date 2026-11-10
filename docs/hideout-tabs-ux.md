# Hideout workspace: module tabs (UX plan)

Issue #953, step 1. Open page panels are tabs above the page area, like editor tabs in VS Code.
The sidebar stays the launcher and module list; the tabs are what is open. Contract fields:
[hideout-contract.md §5.9](hideout-contract.md) (v1.3). Clean-room: written from the contract,
the design doc, `crates/` and public docs; no code under `app/` was read or used.

Mockups (Python + Pillow, [`design/mockups/hideout_tabs_mockups.py`](design/mockups/hideout_tabs_mockups.py),
DARK tokens of `hideout-shell`; the script fails if any drawn label is under 4.5:1, lowest is 5.8:1):
- `review/hideout-tabs/ux-tabs-proposed-strip-states.png`: many tabs, every badge and state
- `review/hideout-tabs/ux-tabs-proposed-empty-and-one.png`: nothing open, one tab
- `review/hideout-tabs/ux-tabs-proposed-overflow.png`: overflow, the tab list, the right-click menu
- `review/hideout-tabs/ux-tabs-proposed-stopped-popout-close.png`: a stopped module, a pop-out, closing with unsaved work

## Research

- **Reuse:** the panel host (#956) already draws one panel by `PanelKey` with its `title` and
  `icon`, and keeps view state by node id (§5.6), so a tab is "which panel the page area shows".
  The module states (§7.8) and pop-outs (§2.5, §5.7) exist in the contract. `egui` has no tab
  strip widget we can take as is; it is a row of selectable buttons plus AccessKit roles.
- **Emulate:**
  - **VS Code editor tabs:** icon + title, close on the active and hovered tab, a dirty dot in
    the close slot that turns into × on hover, middle-click closes, drag to reorder, Close
    others / Close to the right, "Show opened editors" list, Move into new window and back.
  - **Windows Terminal tabs:** tabs shrink before they scroll, scroll arrows and a list button,
    a bell mark for attention, tear-out tabs, Ctrl+Tab in strip order.
  - **Browsers:** Ctrl+1–8 for tab N and Ctrl+9 for the last, Ctrl+W, Ctrl+Shift+T reopens.
  - **WAI-ARIA tabs pattern:** tablist / tab / tabpanel, arrow keys inside the strip.
- **Not taken:** VS Code preview (italic) tabs, pinned tabs, split groups. Each adds a mode the
  user has to learn; revisit only if people ask.

## Scope

In: the hub's own window (the place you see your keyboard, lights and screens; `docs/scope.md`
"one place to see them"). Hub-only: no module code changes beyond the two optional messages.
Out: split views, tab groups, per-tab zoom, modules opening or reordering tabs, tabs for `tile`,
`status`, `settings` or `action` slots (those stay where §5.1 puts them), module windows (§5.7
editors keep their own windows).

## Model

- A tab is one open `page` panel (built-in hub pages such as Modules and Settings too, so every
  page behaves the same). At most one tab per panel.
- **Clicking a sidebar entry** opens its tab to the right of the active one, or switches to it
  if already open. The sidebar highlights the entry of the active tab.
- The hub owns order, the active tab and pop-outs, and saves them in its own files (§2.3).
  Modules only set a badge (`ui.badge`) and hear `ui.visibility`.

## States of the strip

| State | What shows |
|---|---|
| Nothing open | Empty strip; page area: "Nothing open" / "Pick a page in the sidebar." and two shortcuts (Ctrl+K, Ctrl+Shift+T). First start opens the first page instead, so this is only seen after closing everything. |
| One tab | The strip still shows (it names what is open and holds the close button). |
| Many | Left to right in the user's order. Active tab: page background, 2 px orange top edge, full-contrast label. Others: sidebar colour, muted label (6.5:1). |
| Overflow | Tabs shrink to 96 px (title ends in "…", full title in the tooltip), then the strip scrolls (wheel, ‹ ›) and keeps the active tab in view. The list button (˅) shows every tab with its badge and Ctrl+number. |
| Attention | Orange dot on the tab's icon, and on its sidebar entry. Cleared by the module (it knows when the thing was seen). |
| Count | Orange pill with the number, "99+" above 99. |
| Unsaved (dirty) | A dot in the close slot; × on hover. Closing asks (below). |
| Starting, updating, restarting | A thin orange ring round the icon. Tooltip "Updating…" / "Restarting (2 of 5)"; the page header has the §7.8 line ("Your place on this page is kept"). |
| Stopped (crash-stopped, install-failed, needs-hub, quarantined) | Red warning mark replaces the icon. The page shows a card with the §7.8 name, reason and ways out (**Start again**, Start with default settings, Go back to <previous>, Copy diagnostics); the last page drawn stays below, greyed. |
| Module off (disabled) | Tab stays, label muted, page shows "Off" with **Turn on**. |
| Popped out | Tab stays in place, muted, with a ↗ mark; clicking it brings the window forward. Order and Ctrl+numbers don't shift. |

Badge precedence when several apply: stopped mark > unsaved dot > count > attention dot.

## Interactions

- **Open:** click in the sidebar, Enter on a sidebar entry, or Ctrl+K and pick a page.
- **Switch:** click; Ctrl+Tab / Ctrl+Shift+Tab next / previous in strip order (wraps);
  Ctrl+PageDown / Ctrl+PageUp the same; Ctrl+1–8 tab N; Ctrl+9 the last tab.
- **Close:** × on the active or hovered tab, middle-click, Ctrl+W, or right-click > Close.
  Close others and Close to the right ask once if any of them is unsaved. Closing the active tab
  activates the tab to its right, else the left one (VS Code). Ctrl+Shift+T reopens the last
  closed tab at its old place (last 10 kept, this session).
- **Close with unsaved work** (dirty badge): "Close Scenes?" / "Your changes in Scenes aren't
  saved." / **Keep open** (default, Enter and Esc) / Close anyway. The same ask on closing a
  pop-out or quitting from the tray, one dialog listing every unsaved tab.
- **Reorder:** drag a tab; a 2 px orange insertion bar shows where it lands. Keyboard: right-click
  menu Move left / Move right, or Ctrl+Shift+PageUp / PageDown.
- **Pop out:** right-click > Pop out, or drag the tab out of the window. The page moves to a
  `Hideout · <title>` window (§2.5) and the tab stays as a placeholder. **Dock back:** the window's
  Dock back button, closing the window, or dragging its title onto the strip. It returns to its
  old place in the order.
- **Restart:** order, the active tab and pop-outs (with their places per monitor) come back.
  Pages draw from the panel cache at once (§2.4), "Starting…" until the module is ready. Tabs of
  modules that are now removed are dropped silently; disabled ones come back as Off.
- **Module disabled:** its tabs stay, greyed, with Turn on (§5.6: going away greys, it doesn't
  remove). **Module removed:** its tabs close; one note in the status strip: "Art was removed;
  its tab was closed." Ctrl+Shift+T does not bring it back.
- **Visibility to the module:** the active tab and any shown pop-out are `visible`; background
  tabs, minimised windows and the hub in the tray are `hidden`; a closed tab is `closed`. The hub
  stops streams of hidden panels (§5.4).

## Wording

| Where | Text |
|---|---|
| Empty page | "Nothing open" / "Pick a page in the sidebar." |
| Tab tooltip | "<Title>" plus the state line: "Updating…", "Stopped after 5 crashes", "3 new", "Not saved", "In its own window" |
| Right-click | Close, Close others, Close to the right, Pop out, Move left, Move right, Reopen closed tab |
| Pop-out title bar | "Hideout · <Title>", button "Dock back" |
| Unsaved ask | "Close <Title>?" / "Your changes in <Title> aren't saved." / Keep open / Close anyway |
| Removed note | "<Module> was removed; its tab was closed." |

No internal ids anywhere (no panel ids, no `ui.limits`, no state ids like `crash-stopped`).

## Keyboard and screen reader

- Strip is an AccessKit `TabList`, each tab a `Tab` (selected state, position "3 of 7"), the page
  area a `TabPanel` labelled by its tab. Name: title plus state, e.g. "Chat, 3 new", "Scenes, not
  saved", "Art, stopped after 5 crashes", "Images, in its own window".
- Tab order: sidebar, strip (one stop, on the active tab), page. Inside the strip Left / Right move
  focus, Enter or Space activates, Delete closes, Shift+F10 opens the menu; Home / End jump.
- Badge and state changes are announced politely once ("Chat, 3 new"), not on every count change
  within 5 s.
- Colour is never the only signal: the dirty dot, the count text and the warning mark have shapes
  and accessible names. All labels on fills keep ≥ 4.5:1 (checked by the mockup script; the
  shell's `theme.rs` contrast test should cover the tab tokens too).

## UX review of open hub work

### #956 panel host (draft)

Review items, all fixed in the PR:

- **Block, fixed:** the sidebar's selected label is now `theme::nav_label` (`on_accent` on the
  accent fill), from #957.
- **Fixed:** the greyed banner reads the plain §7.8 words through `state_words`, for example
  "Art: Stopped after repeated crashes." The state id is only in the tooltip (and
  `PanelState::details`, for Copy diagnostics). The tab strip should reuse `state_words`.
- **Fixed:** the rejected banner ends at "Art's page couldn't be shown."; the error id is in the
  tooltip only.
- **Fixed:** every palette label is at least 4.5:1 on its row. A selected row uses `on_accent`
  whether or not the entry is available (`theme::palette_label`, with a test over light and dark).
- **Done:** `ui.badge` is stored per page panel (`badge`, `badge_count`; a new instance resets it)
  and `ui.visibility` is sent through `PanelFeed` when the content area changes (`visible` for the
  shown panel, `hidden` for the others, `closed` after `close_panel`; on change only). No tab
  strip yet.
- The single page area keyed by `PanelKey` with title and icon is the right seam for the strip.

### #957 hub first-start fixes (fix/hub-first-start)

- Nav contrast fix is right (`on_accent` on `accent`, 8.0:1 dark) and has a test. Suggest the test
  also run on the palette's selected row and future tabs, through one `selected_label` helper.
- Tray: one log line for the outcome is good; the menu's "No modules yet" and "Open Hideout" /
  "Pause Hideout" are plain words. No block.
- Footprint wakeups: lab scripts only, no user-visible text. No block.

### Router (feat/hideout-router-934, local, not pushed)

User-visible `data.reason` strings: "Waiting for <name>" (matches §7.8), "Needs permission to
open <name>'s <channel>" (channel names are internal ids; prefer the channel's `[[provides]]`
label), "Needs permission for this call" (say which: the method's plain name), "No such channel"
(internal; "<Module> doesn't offer that"), "Key presses are never shared with AI agents" (good).
"hub 1.1 is not served; this hub serves 1.0" must not reach the UI as is; the hub shows "Needs a
newer Hideout" (§7.8). Messages like "initialize first" are developer-only. No block.

## Open questions for the user (via the Hub)

1. Ctrl+Tab order: **strip order** (Windows Terminal, browsers) / most recently used (VS Code).
2. Popped-out tab: **stays in the strip, dimmed** / leaves the strip until docked (VS Code).
3. Built-in pages (Modules, Settings): **open as tabs like modules** / always replace the page.
