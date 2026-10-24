# #298 spike: WebSocket from a Wallpaper Engine wallpaper

Question: can a Wallpaper Engine web wallpaper (CEF, loaded from `file://`) open `ws://127.0.0.1:<port>`, and what `Origin` does it send? Wallpaper Match (#70) needs that before the broker gets inbound TCP/WebSocket (#298).

1. Start the listener (it stops by itself after 10 minutes or 3 connections):

   ```
   python -I scripts/spikes/we-websocket/listener.py
   ```

2. Copy `wallpaper/` to Wallpaper Engine's `projects/myprojects/foxglove-ws-spike/` and pick "Foxglove WebSocket spike" in Wallpaper Engine. A click on the wallpaper (with mouse input on) tries again.
3. Record:
   - **On the wallpaper:** `page origin`, then `OPEN` (green) or `error` / `closed` (red).
   - **From the listener:** the request line, `origin`, and `user-agent`.
4. Put the wallpaper back and delete the spike project.

What it decides: whether the broker can accept the wallpaper's connection at all (`OPEN`), and which `Origin` to check (likely `null` or `file://`); the per-install token stays the real check either way.
