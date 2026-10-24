# Foxglove for SignalRGB

A SignalRGB network plugin (#73) that sends SignalRGB's colour to Foxglove on this PC, so the keyboard's underglow follows your SignalRGB effects. Foxglove holds the lights with a low-priority lease while frames come: its own notifications (mic muted, a focus timer) still show, and the lights go back as they were when SignalRGB stops.

- **What it sends:** one colour for the whole board, as E1.31 (sACN) to `127.0.0.1:5568`, universe 1. Nothing leaves your PC. Per-LED effects come later.
- **When:** each time SignalRGB's colour changes, plus at most one frame a second from SignalRGB's own render loop so Foxglove knows it's still there (UDP has no "closed"). Closing SignalRGB ends the stream at once; if SignalRGB crashes, Foxglove lets go after 2.5 s (E1.31's loss timeout).

## Install

1. In Foxglove's Plugins tab, install and turn on **OpenRGB (E1.31)**. It's the receiver, and it listens only on this PC.
2. Copy `Foxglove.js` to `Documents\WhirlwindFX\Plugins` (create the folder if needed; with OneDrive, the redirected Documents folder).
3. Restart SignalRGB. "Foxglove keyboard" appears as a device; place it on the canvas.

Don't run OpenRGB's E1.31 output to the same port at the same time: both would drive the same lights.

## Notes

- Our own code, MIT, like the rest of this repository. Not made or endorsed by SignalRGB; no SignalRGB code or assets are included.
- Untested in SignalRGB itself so far: its plugin calls `device.setName`, `setSize`, `setControllableLeds` and the network discovery service follow published network plugins, but SignalRGB's documentation doesn't cover them all. `node integrations/signalrgb/test.mjs` checks the packets against Foxglove's receiver and the send-on-change behaviour. `record-packets.mjs` records a short session's packets for the receiver's end-to-end test (`crates/plugin-openrgb-e131/tests/signalrgb.rs`); `--check` (run by `scripts/check.ps1`) fails when Foxglove.js no longer matches the recording.
