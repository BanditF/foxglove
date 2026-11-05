`foxglove-devices` provides crate-owned device identities and capability traits, a caller-driven registry,
thread-safe facts, opt-in key events, and a named actor with timed replies.

Backends implement `Backend` and return `Device` objects exposing their supported capabilities.
Use `EventBus::sink(id)` to publish device facts and events. `Registry` owns open devices;
call `discover()` from the host's hotplug handler. Backends can use `actor::Actor` to own
transport handles on a worker thread.

Enable `mock` for the in-memory backend and `contract` for reusable backend checks.
`contract::check_device` attempts unconfirmed behaviors and rewrites an existing binding;
run it on a disposable test device. Also call `check_binding` with a different valid binding
to verify that writes take effect. It restores the original binding afterward.

Facts and events use unbounded receiver queues; consumers should drain or drop subscriptions.
Only `subscribe(true)` receives typed key events. Key events never become facts.

`pc_lighting` covers lighting that isn't a keyboard: case boards, coolers and Windows Dynamic
Lighting devices. `pc_lighting::discover` matches a HID enumeration against
`lighting-catalog.toml` (VID/PID, protocol, zone names) and the LampArray usage; drivers implement
`LightingDevice` (named zones, static colour, off, identify). Each found device carries
`LightCaps` (colour addressing, effects, brightness, screens), and `pc_lighting::unknown` lists
the rest for a manual add (`test_zone` lights one zone and puts it back). The `hid` feature
adds the hidapi enumeration and a writer that opens the device only while it sends.

`hidpp` is a read-only HID++ 2.0 client (#505) for Logitech devices, direct or behind a
Unifying, Bolt or Lightspeed receiver: framing, the root feature lookup, the feature set, error
replies, and battery reads (0x1000, 0x1001, 0x1004) into the generic `battery::Battery`. It
talks through a `hidpp::Transport`, so its tests run on a scripted fake; it opens no device
itself and sends only "get" requests.

`pc_lighting::openrgb` is an OpenRGB SDK client (#471), written clean-room from OpenRGB's public
protocol document (OpenRGB is GPL-2.0; no code copied, and the GPL-2.0 `openrgb`/`openrgb2`
crates are not used). Off by default (`openrgb::Settings::enabled`), it connects to a running
OpenRGB (`127.0.0.1:6742`), negotiates protocol version 4 or lower, and turns each controller
into a `Source` (a zone per LED, or per OpenRGB zone past 256 LEDs) with a `LightingDevice`
driver (SETCUSTOMMODE once, then UPDATELEDS or UPDATEZONELEDS). Waits are bounded; the
server's DEVICE_LIST_UPDATED push arrives as an event, and reconnecting is a separate opt-in
with a bounded backoff.
