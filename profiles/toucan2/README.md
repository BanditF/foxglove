# Beekeeb Toucan2 profile (#75, groundwork)

This builds in CI; nothing here has run on hardware yet.

- **Firmware:** `.github/workflows/build.yml` job `toucan2` builds `build.yaml` with
  `config-toucan2/` (top level: ZMK's build workflow only takes a one-level config path). Beekeeb's [zmk-keyboard-toucan2](https://github.com/beekeeb/zmk-keyboard-toucan2)
  module (MIT, pinned) supplies the board, its keymap and its own screen widgets (`nice_view_gem`).
  Foxglove adds `raw_hid_adapter` and the `foxglove_link` shield (`boards/shields/foxglove_link`).
  With those, the host link answers HELLO, D5 06 SCREENS, the D7 settings commands and BUILD_INFO.
- **Screens:** one 144x168 1-bit memory LCD on the left (`screens_toucan2.c`). It has no ops yet: the
  PC can't draw into it or read it back until a panel or readback is built for it.
- **Not included:** test hooks (position press, bootloader), art relay, RGB sync. These
  are Corne/nice!view features.
- **Keymap:** `config-toucan2/toucan.keymap`, `toucan_left.conf`, `toucan_right.conf` and `toucan.json` are
  copied from the vendor repo at 6882c95 (MIT, Copyright (c) 2024 Michael Schmidt-Voigt). The app
  reads layers from the keyboard (Studio), not from this file.
