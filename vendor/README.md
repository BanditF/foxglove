# Vendored patches (BanditF/foxglove#1381)

Two crates are copied from crates.io and patched through `[patch.crates-io]` in `../Cargo.toml`,
so AccessKit (the Windows UI Automation tree screen readers read) can be on in the Hideout hub
without the window-creation panic. This is the same pair of fixes the Foxglove app carries for
its own versions in `../app/vendor/` (#1376). Every changed place is marked `HIDEOUT PATCH`.
Licences: both crates are MIT OR Apache-2.0, the same as upstream (licence texts included); see
LICENSING.md. `vendor` is in the workspace `exclude` list, so fmt, clippy and tests of the root
workspace do not treat the copies as members.

- `accesskit_windows` 0.29.2, `src/subclass.rs`: `SetPropW` needs a global atom for its string
  name; when another program has filled Windows' atom table the upstream `.unwrap()` panicked at
  window creation. Now `install` returns the error, the window stays un-subclassed (inert adapter,
  one line on stderr) and `RemovePropW` failures are ignored. `SubclassingAdapter::try_new`
  exposes the error (used by `crates/hideout-shell/tests/accesskit_install.rs`). Dropped from the
  copy: `.cargo_vcs_info.json`, `Cargo.toml.orig`, `.cargo-ok`, `Cargo.lock`, `examples/` (and the
  `[[example]]` table that pointed at it).
- `eframe` 0.33.3, `src/native/glow_integration.rs`: upstream calls `init_accesskit` for the root
  viewport only, so the hub's other windows (immediate viewports: flyout, pop-outs, drag ghost)
  had an empty tree. `initialize_window` now starts the adapter for every non-root viewport as
  well. Dropped from the copy: `.cargo_vcs_info.json`, `Cargo.toml.orig`, `.cargo-ok`,
  `Cargo.lock`.

Drop each copy when an upstream release carries the same fix (upstream issues/PRs are to be filed
at emilk/egui and AccessKit/accesskit; add the links here when they exist). When bumping eframe
or accesskit_windows, copy the new version from the cargo registry the same way and re-apply by
searching for `HIDEOUT PATCH` (or `FOXGLOVE PATCH` in `../app/vendor/`, which is the same change).
