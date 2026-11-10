# Hideout hub idle footprint (#935)

What the empty hub (no modules, window hidden or minimised to the tray) does while idle, every
source of periodic work found by code review of `crates/hideout`, `hideout-shell`,
`hideout-router` and `hideout-registry` (there is no `hideout-panels` crate yet), what was
changed, and what can't go lower. Measured at main 1baa4cc0 by On-call, idle 60 s, window open:
about 80 MB private, 77 MB working set, 0.0 % CPU, 9 to 11 wakeups/s, 20.7 MB dedicated VRAM.

## Wakeup sources

| Source | Before | Now |
|---|---|---|
| eframe event loop | `ControlFlow::Wait` whenever no viewport asked for a repaint (eframe 0.33 `run.rs`); `WaitUntil` only for a `request_repaint_after`. **Hidden to the tray it span in `ControlFlow::Poll`** (see "Hidden hub spinning" below). | The hub runs eframe's app on its own winit loop (`wake::Gate`): with the window hidden, a `Poll` left at `about_to_wait` becomes a wait. The test `shell::tests::an_idle_frame_requests_no_repaint` runs the hub's frame on a bare `egui::Context` and fails if a settled frame asks for any repaint, so a widget that animates or keeps moving can't slip in; `a_hidden_frame_requests_no_repaint` does the same with the hidden flag set, and `wake::tests` pin the control-flow rule. |
| Hub frame (`Hub::update`) | No `request_repaint*` of its own. The palette's text box asks for focus and a blinking caret only while Ctrl+K is open. | Unchanged; the frame body moved to `Hub::draw` so the test above runs it window-free. |
| Tray commands (`on_tray`) | `request_repaint` after every command, so Pause or Open log folder with the window hidden drew a full GL frame into a hidden window. | Repaints only when the window is on screen (`platform::main_window_on_screen`). A hidden window repaints when Windows shows it again (WM_PAINT). |
| tray-icon leave timer | tray-icon 0.24 arms a 15 ms timer (`WM_USER_LEAVE_TIMER_ID`, 6007) on each mouse move over the icon and kills it only on a tick that still holds a fresh cursor position. When the cursor leaves without another move over the icon (always after the tray menu opens: the cursor leaves through the menu) the timer ticks at 64 Hz until the hub exits. | The hub uses no Enter/Leave events, so the Move handler kills the timer as soon as tray-icon arms it (`tray::stop_leave_timer`). A test pins tray-icon to 0.24.x in Cargo.lock, so a bump re-checks the private id. |
| Second-instance signal (`hideout-show`) | `WaitForSingleObject(INFINITE)` on a named event. | Unchanged (no timeout). |
| Router timer (`hideout-router-timer`) | `recv()` with no timeout while no call is pending, `recv_timeout` only up to the next call deadline (30 s call timeout). | Unchanged. New test `crates/hideout-router/tests/idle.rs` serves a pipe, then checks that the timer and accept threads use zero CPU cycles over a quiet second (`QueryThreadCycleTime`). |
| Router accept threads (`hideout-router-accept`, one per pipe) | Overlapped `ConnectNamedPipe` waited with `GetOverlappedResult(bWait = TRUE)`: a kernel wait. | Unchanged; covered by the same test. |
| Router events thread | `for event in rx`: blocks on the channel. | Unchanged. |
| Lab capture (`hideout-capture`, #1002) | Did not exist. | Off on the user's hub: no thread, no pipe. On a lab folder with `HIDEOUT_LAB_CAPTURE=1`, one thread in a kernel wait (`ConnectNamedPipe`); a frame checks an empty slot. A capture costs one frame. See "Lab capture" below. |
| Registry | Read once at start (`Registry::open`); no watcher, no poll. | Unchanged. When the registry needs live updates (H2.2) it must use `ReadDirectoryChangesW` / `FindFirstChangeNotification`, not a timer. |
| Log | `FileLog` writes each line straight to the file; no flusher thread, no timer. | Unchanged. |

No `ControlFlow::Poll`, `sleep` loop or file-watcher poll exists in these crates. eframe's own
`sleep(10 ms)` after a frame on a minimised window only runs when a frame runs, and the idle hub
runs none.

### Hidden hub spinning (main 41c59d38)

On-call measured per thread with the window closed to the tray: the main thread (winit loop and
tray) at 1100 to 1600 context switches/s, the NVIDIA GL driver thread at about 11/s, every
`hideout-*` thread at 0. The Process-class counter read 0 for the process, so the footprint
case's wakeups had missed it.

Cause, from the eframe 0.33.3 and winit 0.30.13 sources:

1. Closing the window runs one frame that sends `ViewportCommand::CancelClose` and hides the
   window (`ShowWindow(SW_HIDE)`). egui's `send_viewport_cmd_to` always calls
   `request_repaint_of`, so that frame ends with a repaint due now.
2. eframe's `check_redraw_requests` (`native/run.rs`) handles a due repaint by setting
   `ControlFlow::Poll`, calling `window.request_redraw()` and dropping the entry. Only a later
   event whose result is `EventResult::Wait` (normally the `RedrawRequested` frame) sets
   `ControlFlow::Wait` again.
3. winit's `request_redraw` is `RedrawWindow(RDW_INTERNALPAINT)`. Windows sends no `WM_PAINT`
   to a hidden window, so `RedrawRequested` never comes.
4. Nothing answers `Wait`, so the loop stays in `Poll`, and winit's `wait_for_messages_impl`
   returns at once for `Poll` (zero timeout, no `MsgWaitForMultipleObjectsEx`): the main thread
   spins through `PeekMessageW` until the window is shown again.

Not the cause, checked: no `timeBeginPeriod`, `SetTimer` or `PeekMessage` loop in the hub
crates; the tray-icon leave timer is killed on arm (above) and a Win32 timer can't tick past
about 100 Hz anyway; tray events come by callback, not a receiver polled from `update`; the
measured `hideout.exe` has no panel feed (`panels: None`), and the panel pump never returns a
deadline in the past (`flush` sets `last = now` for every panel it flushes; asserted in
`hideout-panels/tests/vectors.rs`).

Fix: `hideout_shell::run` builds the winit loop itself and runs `eframe::create_native`'s app
inside `wake::Gate`, which forwards every callback and, at `about_to_wait`, turns a `Poll` into a
wait while the window is hidden (`platform::main_window_drawable`, `IsWindowVisible`). A shown
window keeps eframe's `Poll` (its frame comes and answers `Wait`). Showing the window from the
tray or a second instance repaints it as before (Windows sends `WM_PAINT` on show, plus the
existing `request_repaint`).

Health signal: the gate counts loop wakes (one per `new_events` .. `about_to_wait`) and logs
`event loop: N wakes in 600 s (repaint a, timer b, window c, accesskit d, device e, poll f,
other g)` every 10 minutes; every wait ends by the next report, so an idle hub logs the line on
time (one wake per 10 minutes is its cost). A hidden, idle hub should log a few wakes per report,
not hundreds of thousands. The causes say whose wakes they are (#1000, `wake::Cause`): `repaint`
is eframe's repaint event (our wakers: the panel feed, stream readers, the show signal, tray
commands; and egui), `timer` a wait the loop asked for (a frame's `request_repaint_after`, the
report), `window` and `device` winit events, `accesskit` a screen reader or UI Automation client
walking the window, `poll` a `Poll` turn, and `other` a message winit handled itself or one for
another window of the thread (the tray icon's, IME, COM) that reaches no handler of ours.
`HIDEOUT_WAKE_REPORT_S` (5 to 600 s) shortens the period; the lab's footprint case uses 15 s.

### What is left, and why it may not reach < 1/s

The hub's own threads are now all in infinite kernel waits at idle (two of them proved by a
test, the rest by review). The 9 to 11 wakeups/s On-call measured were taken with the window
**open** and no tray hover, so neither fix above explains them; the remaining sources are
outside the hub's code:

- **The OpenGL driver.** eframe's glow backend loads the vendor's WGL driver (nvoglv64 on the
  lab PC), which starts its own worker threads (threaded optimisation, present and residency
  tracking). They are not the hub's to stop, and eframe 0.33 has no way to drop the GL context
  while the window is hidden.
- **UI Automation clients.** With `accesskit` on, any UIA client on the PC (Logi Options+,
  PowerToys, the On-call's own UIA probe) sends WM_GETOBJECT to the visible window; each is a
  wakeup. This is accessibility, so it stays.
- **Windows' text input (TSF/IME) and DWM** messages to a visible window.

On-call should re-measure with the window **hidden to the tray** (the target case, never
measured so far) and with it minimised; if wakeups stay above 1/s there, record per-thread
context switches (`Win32_PerfRawData_PerfProc_Thread`, by thread id) to tell the driver's
threads from the hub's (the hub's threads carry the names `hideout-*`).

## Memory

| Item | Before | Now |
|---|---|---|
| Renderer | glow (OpenGL), the lighter eframe backend: wgpu adds the wgpu stack and a DX12 or Vulkan device on top. MSAA 0, depth 0, stencil 0 (eframe defaults). | Unchanged. |
| eframe features | `default-features = false`: no wgpu, no persistence, no bundled fonts. | Unchanged. |
| Fonts | `segoeui.ttf`, `consola.ttf`, `seguisym.ttf` read into owned `Vec`s (3.9 MB on the lab PC). epaint clones an owned font when it parses it (`FontVec::try_from_vec(bytes.clone())`), so about 7.8 MB of private heap. | Mapped read-only (`platform::map_file`) and handed to egui as `FontData::from_static`: epaint borrows them (`FontRef`), the pages are file-backed and shared, and only touched pages enter the working set. Expected about 7 to 8 MB less private memory. |
| Font atlas | egui's atlas, grown on demand for the glyphs used; a few MB of CPU image plus one GPU texture. | Unchanged; the hub uses two text sizes and few glyphs. |
| Images, caches | No image loaders, no texture caches in the shell. | Unchanged. |

### What can't go lower, and why

Most of the rest is the GPU stack: the OpenGL ICD and its allocations, the swap chain and the
GL context (20.7 MB dedicated VRAM, plus the driver's private heap in the process). eframe 0.33
creates the GL context once with the window and offers no call to destroy it while the window
is hidden and remake it on show; doing that would mean leaving eframe for a hand-made
winit + glow loop, which the visuals don't need. A software renderer would trade the driver's
memory for CPU on every frame and is not available through eframe's glow path on Windows.
Trimming the working set while hidden (`EmptyWorkingSet`) would only move pages to the
standby list, not lower private bytes, so it isn't done.

### Limits and pop-outs (11/10)

On-call's runs on 2bc31b90 (after the #1015 spin fix): CPU 0.03–0.17 %, loop wakes about 1.6/s
hidden and 4.4/s shown, nearly all with cause `other` (window messages winit handles itself or
that go to another window on the UI thread). At that cost the gate's limits are at most 2 loop
wakes/s hidden (no flyout) and 5/s shown (`tests/hil/hideout-footprint-baseline.json`). The
report now splits `other` by window message (#1018, below).

One pop-out window with six modules idles at 92.8 MB private (On-call, 2bc31b90), against about
75 MB for the hub alone. The pop-outs already share the main window's GL context (#1018, below).

### `other` wakes by message (#1018)

The wake report names the messages its `other` wakes handled, after the causes:

```
event loop: 24 wakes in 15 s (repaint 0, timer 1, window 0, accesskit 0, device 0, poll 0,
other 23); other (0x0113 WM_TIMER#6007 tray 18, 0x003D WM_GETOBJECT main sent 4,
0xC2FA <registered name> 'Winit Thread Event Target' 2, more 1, unseen 2)
```

Each entry is the message id and name (registered messages, 0xC000 and up, by their registered
name; `WM_USER+n` and `WM_APP+n` as offsets), the timer id for `WM_TIMER`, the window
(`main`, `tray` (tray-icon's hidden window), `flyout`, `thread` for a thread message, else the
window's class in quotes, `gone` once it was destroyed), `sent` for a message another thread
sent, and the message count over the period, most first, top five. `more` is the messages past
the top five, `unseen` the `other` wakes neither hook saw a message for. The lab parser
(`footprint_compare.WAKE_LINE`) reads the causes before the `;` as before.

How it counts (no admin, no DLL, nothing in another process): winit's message hook
(`with_msg_hook`, `platform::note_queued`) sees each message the loop takes from the queue
(posted messages, `WM_TIMER`, `WM_PAINT`); a thread-local `WH_CALLWNDPROC` hook
(`platform::install_message_hooks`) sees each message another thread sends (`InSendMessageEx`;
the hook's wParam can't tell, Windows' own sends while the thread creates a window come with
the same value), which `PeekMessageW` delivers inside the wait without returning it. Both count
into a fixed 8-kind table for the wake (`wake::MsgTally`); at `about_to_wait` an `other` wake's
table folds into the period's 24-kind table, and anything else is dropped. No allocation per
message or wake, no timer, no wake of its own; names are looked up once per report.
Tests: `wake::tests::the_report_names_the_top_messages_of_the_other_wakes` pins the line, and
the real-loop test runs a thread timer and a send from another thread to a window of the loop's
thread through winit and both hooks.

**Which are ours, from code** (On-call's 2bc31b90 runs: hidden ~1.6 wakes/s, shown ~4.4/s,
nearly all `other`):

| Message | Ours? | Why |
|---|---|---|
| `WM_TIMER#6007 tray` | tray-icon's, killed on arm | The leave timer (#935): `tray::stop_leave_timer` kills it in the Move handler. Only while the cursor moves over the icon. A count of more than a few per report means the kill no longer works (a tray-icon bump; `Cargo.lock` is pinned to 0.24.x by a test). |
| `WM_USER+…` / registered id to `tray` | the shell's | Explorer's tray callbacks (mouse over or clicks on the icon, `TaskbarCreated` after an Explorer restart). Only with the cursor on the icon. |
| any `WM_TIMER` to `main`, `thread` or a pop-out | ours if any | The hub crates call no `SetTimer` (searched: only the tray kill and the lab driver's `PostMessageW`, lab only). eframe and winit set none on Windows; egui's waits are the loop's own `WaitUntil` (`timer` cause, not `other`). |
| `WM_PAINT` to `main` | not in `other` | A paint of our window runs eframe's frame (`RedrawRequested`), so it counts as `window`. Avoidable redraws would show there, not here. |
| `WM_PAINT` to `'Winit Thread Event Target'` | winit's, same wake | winit calls `RedrawWindow(RDW_INTERNALPAINT)` on its thread window for every message it gets there, and handles the paint in the same dispatch: no extra wake. |
| `WM_GETOBJECT … sent` | the OS's (UI Automation) | A UIA client (Logi Options+, PowerToys, Narrator, the On-call probe) walking the window; accessibility stays. |
| `WM_DWM…`, `WM_SETTINGCHANGE`, `WM_DEVICECHANGE`, `WM_DISPLAYCHANGE`, `WM_POWERBROADCAST`, `WM_THEMECHANGED` | the OS's | DWM and system broadcasts; every top-level window gets them, hidden ones too (winit's thread window is one). |
| `'MSCTFIME UI'`, `'IME'`, registered `MSUIM.*` | the OS's (TSF/IME) | Text services' own windows on the UI thread. |
| `'OleMainThreadWndClass'` | the OS's (COM) | COM's apartment window: OLE drag and drop (winit's), toasts (`notify`). |
| `unseen` | the OS's, or a driver | No message reached either hook: a send handled before the hook could see it, an APC, or the GL driver's own wait on the thread. |

So by code review nothing of the hub's own posts or timers is left at idle: the one source that
was ours (the 64 Hz tray-icon timer) was fixed in #935, and a regression would now show by name.
No fix in this change; On-call's next run of case 7 names the rest (re-measure step 7 below),
and if a row of the table turns out to be ours and large, it gets its own issue.

### Pop-outs and the GL context (#1018)

Finding, from the eframe 0.33.3 source (`native/glow_integration.rs`): eframe's glow backend
already makes **one** GL context (`GlutinWindowContext::current_gl_context` /
`not_current_gl_context`) and **one** `egui_glow::Painter` (so one font atlas and one set of
textures) for all viewports. Each viewport, the hub's pop-outs included (they are immediate
viewports, `show_viewport_immediate`), gets only its own winit window and its own
`glutin::surface::Surface<WindowSurface>` (`initialize_window` → `create_window_surface`); a
frame makes the shared context current on that viewport's surface (`change_gl_context`) and
swaps it. So there is nothing to share that isn't shared already, and nothing to build.

What a pop-out costs, then: the window itself, its WGL drawable (the driver's front and back
buffers at the window's size, plus the driver's per-drawable bookkeeping in the process), its
`egui_winit::State` (clipboard, IME, an AccessKit adapter with its own tree), and DWM's
redirection surface (in DWM, not in the hub). Estimate: about 4 to 8 MB private per pop-out
window on the lab's NVIDIA driver, the rest of the 17.8 MB difference being the six modules
themselves (their panels, text and layout), if the 75 MB reading (hub alone) had none of
them docked. Ways below that all leave eframe's model: one window for all pop-outs, or a
renderer without a per-window GL drawable; neither is a small or safe change, so none is made.
On-call can settle the split with one run (re-measure step 8 below).

### The tray flyout (#991)

A tray click opens the flyout: an egui immediate viewport, so its frames run inside the main
window's frame, and eframe asks the main window for a frame whenever the flyout needs one. A
hidden main window gets no `WM_PAINT`, so while the flyout is up `wake::Gate` runs that due frame
itself (`wake::hidden_pass`: eframe left `Poll`, the main window is hidden, and `flyout::live()`).
The flag goes up on the tray click and down on the first frame after the flyout closed (that
frame drops its window). With the flyout closed nothing changes: the hidden hub waits as above.
Tests: `wake::tests::a_hidden_window_runs_frames_only_while_the_flyout_is_up` pins the rule, and
`shell::tests::the_flyout_opens_settles_and_closes_while_the_hub_is_hidden` runs the hidden hub's
frame with the flyout open and checks it settles (no repaint asked) and that the closed flyout
leaves a hidden frame that asks for nothing. Hover and clicks in the flyout cost a frame of the
hidden main window each, as a shown window's would; a restart countdown on a tile asks for one
frame a second only while it is on screen.

A tray click also kicks one hidden frame (#1019): `flyout::kick` on the click, taken by the gate
at the end of that wake (`wake::hidden_pass`'s `kicked`). Without it a hidden hub never drew the
flyout: the frame that hid the window left egui holding a repaint request eframe could not answer
(no `RedrawRequested` for a hidden window), and egui calls eframe's repaint callback only for a
sooner request, so the click's own `request_repaint` stopped inside egui and eframe's flow stayed
`Wait`. The first frame resets egui's request, so the flyout's own requests reach eframe again;
a click costs one frame plus the few that place and show the flyout, then the hub waits.
Test: `shell::tests::a_tray_click_draws_the_flyout_while_the_main_window_is_hidden`.

### The hidden hub at ~97 wakeups/s (#1000, main d333bd52)

On-call measured the hidden hub at 91 to 97 wakeups/s (main thread 63 to 68/s, CPU 0.10 to
0.12 %) after #995 to #997. A hand check on d333bd52 with the flyout never opened read the main
thread at 100 switches/s while the hub logged `event loop: 1182 wakes in 10 min` (about 2/s): the
main thread is woken about 100 times a second **without** winit's wait returning, so the flyout
is not the cause and the source is still open (#1000). The hub crates install no
`SetWindowsHookEx`, `SetWinEventHook`, `RegisterRawInputDevices`, `SetTimer`, device or session
notification, or message-only window of their own, and with the flyout closed no path runs
frames (read through eframe 0.33.3, egui 0.33.3, winit 0.30.13, tray-icon 0.24.2):

- A background `request_repaint` (the panel feed's waker, stream readers) wakes the hidden hub
  at most once: egui only calls the repaint callback when the new delay is shorter than the
  stored one, and no frame runs to reset it.
- `wake::hidden_pass` needs `flyout::live()`, which only a tray click sets.
- The tray-icon leave timer is still killed as it is armed.

Three ways an open flyout could keep the hidden hub drawing are now closed:

1. **The focus hand-off failed.** A hub that may not take the foreground (started by a script,
   no recent input of its own) shows the flyout without the focus, so it could never close on
   focus loss. Now a flyout that was shown but never had the focus closes after
   `flyout::FOCUS_GRACE` (750 ms), with one wake to look (`flyout::focus_check`).
2. **A frame loop.** Hidden frames the flyout asks for right away are bounded: after
   `wake::HIDDEN_STREAK` (30) back to back with no event between them, the gate waits for the
   next event (input, a timer the frame asked for, a wake). Only a `Poll` wake continues the
   streak (`wake::next_streak`, `wake::side_frames`).
3. **Placing again for a rounding.** The flyout is placed (resized) again only for a change of
   at least one whole physical pixel (`flyout::needs_place`); each placing resizes, and a resize
   brings a frame, so a sub-pixel difference at a fractional scale could loop.

Tests: `flyout::tests::a_flyout_that_never_got_the_focus_closes_after_the_grace`,
`flyout::tests::it_is_placed_again_only_for_a_whole_pixel`,
`wake::tests::hidden_frames_stop_after_a_streak_and_start_again_on_an_event`,
`shell::tests::a_flyout_that_never_got_the_focus_closes`,
`shell::tests::an_idle_open_flyout_settles_to_no_repaint`, and
`shell::tests::a_hidden_hub_with_a_restarting_module_requests_no_repaint` (a module restarting
while hidden: one wake per router event, and the frames it brings ask for nothing; a crash bar
countdown must only count down while its page or the flyout is on screen).

### #1000 resolved: loop wakes are ours and limited, thread readies are recorded

The readings above mix two things. **Loop wakes** are the hub's own: each is a return from
winit's `MsgWaitForMultipleObjectsEx` and one turn of our handlers. **Thread readies** (context
switches) also count every time Windows runs one of the hub's threads for its own work and puts
it back to sleep without the wait returning: the NVIDIA OpenGL driver thread (~11/s even on the
#967 build), TSF/IME and COM apartment work on the UI thread, and messages sent to the thread's
windows from other processes. The ~100 readies/s on the main thread of d333bd52 never reached
the loop (it counted ~2 wakes/s), so they are OS and driver work, not ours. On-call's harness
listed and closed windows during case A (window enumeration sends messages to every top-level
window), which may have added to them.

So the footprint gate (`scripts/lab/hideout_footprint.py`) now limits **loop wakes**: at most
1/s with the window hidden to the tray and no flyout (`--state hidden`, the default; the case
closes the main window by `WM_CLOSE`, as its X does), and the shown window has its own limit
(`--state shown`, `loop_wakes_shown_per_s`). The reading is the hub's own counter: the case
starts the hub with `HIDEOUT_WAKE_REPORT_S=15` and reads the report lines written during the
window from `logs\hideout.log` under its `HIDEOUT_CONFIG_DIR`, dropping the first (its period
began before the window). That needs no admin, no request channel into the hub and no wake beyond
the report itself (0.07/s, inside the reading), and On-call reads the same line on any hub. The
causes go to the result as `loop_wake_causes`. Context switches (`wakeups_per_s`) and
`top_threads` stay in the result as **INFO**, never compared or failed.

**The ~2 loop wakes/s on d333bd52** (1182 in 10 min; #967 measured about 8): read through our
code, none of our wakers can fire that often while hidden with the flyout closed. The router,
supervisor and panel feed reach the loop only through `request_repaint`, and egui calls the
repaint callback once until a frame runs, which a hidden hub without the flyout never does
(`shell::tests::a_hidden_hub_with_a_busy_feed_wakes_the_loop_once_per_burst`: 20 feed events
while hidden are one loop wake, and the frames after it ask for no repaint). The report timer is
one wake per 10 minutes, and no other timer of ours arms while hidden (`flyout::focus_check` only
while an unfocused flyout is up, the tray-icon leave timer is killed on arm). So the cause is not
found in our code and is left to the per-cause count: the next hidden run says whether the wakes
are `other` (an OS message to a window of the UI thread: 1182 is close to a 530 ms caret-blink or
similar system timer, 1132 per 10 min, but that is unverified), `accesskit` (a UI Automation
client such as the harness walking the window) or ours (`repaint`, `timer`). If they are ours,
#1000 comes back with the cause named.

The gate's case, window-free: `wake::tests::a_hidden_hub_without_the_flyout_wakes_only_for_events`
(a hidden hub, no flyout, a feed event every 4 s: every flow eframe leaves is a wait until the
next event or report, 18 wakes in 60 s, under 1/s), `wake::tests::a_wake_is_named_by_what_it_dispatched`
and `wake::tests::wakes_are_reported_once_per_ten_minutes_by_cause`; the compare and the log
reading are in `scripts/test_hideout_footprint.py`.

### The P0 spin on main cde0b451 (#1000)

On-call's case 7 on cde0b451 read ~4.2 M loop wakes/s hidden and ~3.5 M shown, 70–90 % of a core,
every wake `timer`. The cause was the gate's own previous answer, not a frame or a waker: winit
keeps the control flow from turn to turn, and eframe sets it only while it has a repaint due
(`check_redraw_requests` in eframe's `native/run.rs`). On a turn where eframe had nothing due,
`about_to_wait` read back the gate's last flow, the wait until the wake report, which the report
had just made a passed deadline, and `settle` kept it (`t.min(report_at)` of a passed `t` is that
`t`). winit returned at once with `ResumeTimeReached` on every turn, forever. The bug was older
than #1004–#1009; it showed only once the first report had passed, and d333bd52's 63 s reading
ended long before its 10 min report. #1007 set `HIDEOUT_WAKE_REPORT_S=15` in the footprint case,
so the report passed inside the window.

The fix: `Gate::new_events` resets the flow to `Wait` before eframe's `new_events`, so each turn's
flow is eframe's own decision. Behind it `wake::guard` never hands winit `WaitUntil(t)` with
`t <= now`: with no frame able to run (hidden, minimised, not drawable) it waits until the report;
a drawable window gets at most `PAST_TURNS` (3) such turns without a `RedrawRequested`, then waits
until the report too. A firing guard logs `event loop: wait guard fired: …` at most once an hour.
Tests: `wake::tests::the_real_loop_sleeps_when_eframe_sets_no_flow` drives the real `Gate` on a
real winit loop (its hidden message window only) with a mock eframe that asks for one repaint and
then nothing; on cde0b451's gate it ran 1,000,002 turns in 0.68 s (1.46 M/s), now under 20/s.
`wake::tests::a_passed_deadline_every_turn_does_not_spin` runs the per-turn decision against an
eframe answering a passed deadline every turn for 1 s of fake time, hidden and shown.

Re-open #1000 only if a cheap non-admin check (the per-cause count, `top_threads` with thread
descriptions) shows our own threads causing the readies, or idle CPU or power (for example
`powercfg /srumutil` energy per app, or CPU % over a long hidden window) shows a real cost.

## What On-call re-measures

`scripts/lab/hideout_footprint.py` on the PR build, sets "none" and "default":

1. As today (window open): private MB should drop by about 7 MB; wakeups unchanged.
2. Window closed to the tray (click X) before the window: wakeups and CPU, the target case.
3. After opening and closing the tray menu once: wakeups must not jump by about 64/s (the
   tray-icon timer fix).

Wakeups are the Thread-class `Context Switches/sec` summed over all the hub's threads (the
Process-class counter reads 0 for it), and each result records `top_threads`: the three
threads that switch most, with id, description and switches/s.

4. Window closed to the tray, then the flyout opened and closed (click the icon, then Esc):
   wakeups and CPU must come back to case 2 within a few seconds of closing it.

5. (#1000) Window closed to the tray by its X, **the flyout never opened**, and the harness
   closing only the main window (one window): main thread under 1 switch/s, total wakeups as in
   case 2 of the #967 re-measure (about 11/s). This tells whether the no-flyout hub is clean.
6. (#1000) Window closed to the tray, then the flyout opened by a tray click and dismissed (Esc,
   or a click elsewhere), and also once opened by a click from a script with no click after it
   (no focus): within about a second of each it closes by itself or is gone, and wakeups come
   back to case 5. The hub log says `tray flyout closed: it never got the focus` for the second.

After the hidden-hub fix: window closed to the tray, main thread under 1 switch/s (was 1100 to
1600), and the log shows `event loop: N wakes in 10 min` with N in the tens at most.

7. (#1018) Case 7 hidden and shown, as on 2bc31b90: the same limits (≤ 2 loop wakes/s hidden,
   ≤ 5/s shown), and copy the `; other (…)` part of two report lines per state into #1018. A
   `WM_TIMER` to `tray`, `main`, `thread` or a pop-out above a few per report is ours.
8. (#1018) Private MB for the hub with the same six modules docked (no pop-out), then with one
   of them popped out: the difference is the pop-out window's own cost.

## Lab capture (#1002)

`PrintWindow` and `BitBlt` give a blank image of the hub: it draws with OpenGL (eframe's glow
backend). A lab hub can instead read back the frame it has just drawn and write it as a PNG.

How it works: `hideout.exe --capture <out.png>` connects to the lab hub's capture pipe
(`\\.\pipe\hideout-capture-<user><folder suffix>`, current user only, no remote clients) and sends
one line. The hub runs one frame; a paint callback on its topmost layer reads the back buffer
(`Painter::read_screen_rgba`, as eframe's own screenshot does) of the main window and, if it is
open, the tray flyout. The pipe thread writes the PNGs and answers. `ViewportCommand::Screenshot`
is not used: eframe 0.33's glow backend drops it for immediate viewports (the flyout), and its
answer only comes with a later frame. Code: `crates/hideout-shell/src/capture.rs`.

Cost: the user's hub (default folder) never serves captures, whatever the environment says.
A lab hub serves them only with `HIDEOUT_LAB_CAPTURE=1`: one thread blocked in
`ConnectNamedPipe`, nothing per frame but an empty-slot check, no timer, no extra frame. Each
capture costs one frame; the pipe thread waits at most 5 s for it (fallback only). Tests:
`shell::tests::a_capture_adds_one_readback_only_to_the_frame_that_answers_it` (no read-back and
no repaint while unused, one read-back on the asked frame only),
`shell::tests::a_rendered_frame_goes_through_the_capture_png_writer` (a headless hub frame
through the same PNG writer), and `capture::tests::a_capture_round_trips_over_the_pipe`.

### On-call usage

1. Start the lab hub with its own folder and the switch (both are needed):

   ```powershell
   $env:HIDEOUT_CONFIG_DIR = 'C:\lab\hub-1'      # any lab folder; never the default
   $env:HIDEOUT_LAB_CAPTURE = '1'
   Start-Process <path>\hideout.exe
   ```

   `hideout.log` in that folder says `capture: lab capture on, \\.\pipe\hideout-capture-...`.
2. Bring up what the proof needs (the window on screen; the flyout open for a flyout shot).
3. From a shell with the same `HIDEOUT_CONFIG_DIR` (the pipe name follows the folder), capture:

   ```powershell
   $p = Start-Process <path>\hideout.exe -ArgumentList '--capture','C:\lab\shots\hub.png' `
       -NoNewWindow -Wait -PassThru -RedirectStandardOutput C:\lab\shots\capture.txt
   $p.ExitCode                                   # 0: every window was written
   ```

   The main window goes to `hub.png`, an open flyout to `hub-flyout.png` beside it. Each answer
   line starts `ok <window> <w>x<h> <path>` or `error ...`; `hideout.log` gets one
   `capture: <window> <w>x<h> to <path>` line per file. The client opens no window and starts
   nothing; the release `hideout.exe` is a GUI-subsystem program, so its answer is only seen
   with the output redirected as above (`Start-Process -Wait`, or `subprocess.run(...,
   capture_output=True)` from Python).
4. Errors: `nothing on screen` (the window is hidden or minimised and the flyout is closed: run
   `hideout.exe` on the same folder to show the window, then capture again); `no lab hub with
   capture on` (the hub isn't running on that folder, or started without
   `HIDEOUT_LAB_CAPTURE=1`); `<window> was not drawn within 5 s` (the window went away).

The PNGs are opaque RGBA at the window's size in physical pixels (client area only: the title
bar is drawn by Windows, not by the hub). Pass a `.png` path; relative paths are taken from the
client's working directory.

1600), and the log shows `event loop: N wakes in 600 s (...)` with N in the tens at most.

7. (#1000, the gate) `hideout_footprint.py --app <hideout.exe>` (hidden, the default) on this
   PR's build, sets "none" and "default", with no other tool listing windows during the window:
   `loop_wakes_per_s` at most 1 (PASS), and the `loop_wake_causes` of the result posted on #1000.
   Then once with `--state shown --update-baseline` to set the shown window's loop-wake value.
   Context switches and `top_threads` are INFO.
