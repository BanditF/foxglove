# Keeping app/ in step with KeyPeek

`app/` is [KeyPeek](https://github.com/srwi/keypeek) by Stephan Rumswinkel (GPL-3.0), brought in
with `git subtree` from BanditF/keypeek `monitor-select` (upstream `master` plus the monitor
picker), history included, and renamed to Foxglove.

## What we changed, and keep small

Only names, so upstream changes merge cleanly:

- `Cargo.toml`: package `foxglove`, version, description, repository, bundle name/identifier
- Window titles, tray tooltip, log prefixes, the Wayland layer namespace
- Settings dir: `ProjectDirs::from("", "", "Foxglove")`. Until Foxglove saves its own,
  `Settings::load` reads KeyPeek's `settings.ini`; KeyPeek's file is never written
- Settings footer: link to this repo plus "based on KeyPeek by Stephan Rumswinkel"
- `cargo-appimage.desktop`, `flake.nix`, `resources/macos-info-plist-ext.xml`: names

New Foxglove code goes in new modules (`src/foxglove/`) rather than into upstream files.
`app/` keeps its own Cargo workspace and `Cargo.lock`, like upstream, so lockfile updates merge too.

## Pulling upstream

```bash
git fetch https://github.com/srwi/keypeek master
git subtree pull --prefix=app https://github.com/srwi/keypeek master
```

Expect conflicts only in the renamed lines above; keep our names and take upstream's logic.
Then build and test (`cargo test --manifest-path app/Cargo.toml`) and check the overlay.

When upstream merges its own monitor picker (srwi/keypeek#49), ours (`monitor-select`) will
conflict with it in `settings.rs`, `ui_settings.rs` and `eframe_host.rs`: take upstream's.

## Sending a change upstream

```bash
git subtree split --prefix=app -b keypeek-export
```

That branch has `app/` at its root with history; cherry-pick the generic commit onto a branch
of BanditF/keypeek, revert our renames in it, and open the PR to srwi/keypeek from there.
