`foxglove-scene` is the core of Foxglove's scenes (#481): light that travels through the room.
It has no UI and makes no OS calls, so everything in it runs headless in tests.

- `space`: room space, in millimetres, as the user sees the desk from the chair: x to the right,
  y down, the origin at the primary monitor's top-left corner. The monitors' wall and the desk
  in front of it are one flat canvas (the desk folded down below the screens).
- `room`: `RoomMap::build` places every device from plain inputs: monitor rectangles and DPI
  from Windows, the keyboard halves (whole strip for now), the mouse, the case's zones and any
  other lighting device. Defaults need no user input; a user's drag is an offset per device in
  `Layout`, which the app stores.
- `clock`: one clock in milliseconds since an epoch on the system clock (Unix ms). A clock can
  start now or align to an external epoch (the start time Foxglove hands a wallpaper), so both
  compute the same phase: `phase = ((now_ms - epoch_ms) mod period_ms) / period_ms`.
- `effects`: pure `f(position, time) -> colour`: scanline, sweep, ripple, breathe, gradient
  and noise, mixed in Oklab (`palette`).
- `scheduler`: renders frames only while a scene runs and nobody is away or paused. With no
  scene there is no thread and no timer; while one runs, the only timer is the frame interval
  (each device at most 30 fps).

Device outputs (#485), the Scenes page (#487) and presets (#488) build on this crate.
