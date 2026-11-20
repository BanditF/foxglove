# Vendored patches (BanditF/foxglove#1376)

Two crates are copied from crates.io and patched through `[patch.crates-io]` in `../Cargo.toml`,
so AccessKit (the Windows UI Automation tree screen readers read) can be on without the crash
that made commit 23b354fa turn it off. Upstream was checked on 2026-11-20: eframe 0.37.0 and
accesskit_windows 0.35.2 still have both gaps, so a version bump does not fix them.
Every changed place is marked `FOXGLOVE PATCH`. Licences: MIT/Apache-2.0 (eframe: MIT OR Apache-2.0;
accesskit_windows: MIT OR Apache-2.0), the same as upstream; see LICENSING.md.

- `accesskit_windows` 0.32.1, `src/subclass.rs`: `SetPropW` needs a global atom for its string
  name; when another program has filled Windows' atom table the upstream `.unwrap()` panicked at
  window creation. Now `install` returns the error, the window stays un-subclassed (inert adapter,
  one line on stderr) and `RemovePropW` failures are ignored. `SubclassingAdapter::try_new` exposes
  the error (used by the test in `app/src/a11y_tree.rs`).
- `eframe` 0.36.1, `src/native/glow_integration.rs`: upstream calls `init_accesskit` for the root
  viewport only, so a second window (the Settings window is an immediate viewport) had an empty
  tree. `initialize_window` now starts the adapter for every non-root viewport as well.

Drop each copy when an upstream release carries the same fix (upstream issues/PRs are to be filed
at emilk/egui and AccessKit/accesskit; add the links here when they exist). When bumping eframe
or egui, re-apply by searching for `FOXGLOVE PATCH`.
