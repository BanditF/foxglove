# Hideout hub idle footprint (#935)

What the empty hub (no modules, window hidden or minimised to the tray) does while idle, every
source of periodic work found by code review of `crates/hideout`, `hideout-shell`,
`hideout-router` and `hideout-registry` (there is no `hideout-panels` crate yet), what was
changed, and what can't go lower. Measured at main 1baa4cc0 by On-call, idle 60 s, window open:
about 80 MB private, 77 MB working set, 0.0 % CPU, 9 to 11 wakeups/s, 20.7 MB dedicated VRAM.

## Wakeup sources

| Source | Before | Now |
|---|---|---|
| eframe event loop | `ControlFlow::Wait` whenever no viewport asked for a repaint (eframe 0.33 `run.rs`); `WaitUntil` only for a `request_repaint_after`. | Unchanged. The test `shell::tests::an_idle_frame_requests_no_repaint` runs the hub's frame on a bare `egui::Context` and fails if a settled frame asks for any repaint, so a widget that animates or keeps moving can't slip in. |
| Hub frame (`Hub::update`) | No `request_repaint*` of its own. The palette's text box asks for focus and a blinking caret only while Ctrl+K is open. | Unchanged; the frame body moved to `Hub::draw` so the test above runs it window-free. |
| Tray commands (`on_tray`) | `request_repaint` after every command, so Pause or Open log folder with the window hidden drew a full GL frame into a hidden window. | Repaints only when the window is on screen (`platform::main_window_on_screen`). A hidden window repaints when Windows shows it again (WM_PAINT). |
| tray-icon leave timer | tray-icon 0.24 arms a 15 ms timer (`WM_USER_LEAVE_TIMER_ID`, 6007) on each mouse move over the icon and kills it only on a tick that still holds a fresh cursor position. When the cursor leaves without another move over the icon (always after the tray menu opens: the cursor leaves through the menu) the timer ticks at 64 Hz until the hub exits. | The hub uses no Enter/Leave events, so the Move handler kills the timer as soon as tray-icon arms it (`tray::stop_leave_timer`). A test pins tray-icon to 0.24.x in Cargo.lock, so a bump re-checks the private id. |
| Second-instance signal (`hideout-show`) | `WaitForSingleObject(INFINITE)` on a named event. | Unchanged (no timeout). |
| Router timer (`hideout-router-timer`) | `recv()` with no timeout while no call is pending, `recv_timeout` only up to the next call deadline (30 s call timeout). | Unchanged. New test `crates/hideout-router/tests/idle.rs` serves a pipe, then checks that the timer and accept threads use zero CPU cycles over a quiet second (`QueryThreadCycleTime`). |
| Router accept threads (`hideout-router-accept`, one per pipe) | Overlapped `ConnectNamedPipe` waited with `GetOverlappedResult(bWait = TRUE)`: a kernel wait. | Unchanged; covered by the same test. |
| Router events thread | `for event in rx`: blocks on the channel. | Unchanged. |
| Registry | Read once at start (`Registry::open`); no watcher, no poll. | Unchanged. When the registry needs live updates (H2.2) it must use `ReadDirectoryChangesW` / `FindFirstChangeNotification`, not a timer. |
| Log | `FileLog` writes each line straight to the file; no flusher thread, no timer. | Unchanged. |

No `ControlFlow::Poll`, `sleep` loop or file-watcher poll exists in these crates. eframe's own
`sleep(10 ms)` after a frame on a minimised window only runs when a frame runs, and the idle hub
runs none.

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

## What On-call re-measures

`scripts/lab/hideout_footprint.py` on the PR build, sets "none" and "default":

1. As today (window open): private MB should drop by about 7 MB; wakeups unchanged.
2. Window closed to the tray (click X) before the window: wakeups and CPU, the target case.
3. After opening and closing the tray menu once: wakeups must not jump by about 64/s (the
   tray-icon timer fix).
