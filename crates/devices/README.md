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
