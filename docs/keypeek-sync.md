# Keeping app/ in step with KeyPeek

`app/` is [KeyPeek](https://github.com/srwi/keypeek) by Stephan Rumswinkel (GPL-3.0), brought in
with `git subtree` from BanditF/keypeek `monitor-select` (upstream `master` plus the monitor
picker), history included, and renamed to Foxglove.

## `app/` is a hard fork (since #579)

The fork point is upstream [`d7f5a89`](https://github.com/srwi/keypeek/commit/d7f5a89)
(2026-09-05) plus our monitor picker. Since then upstream has rebuilt the whole app (crate split
`2aaccf7`, unified drivers and transport `1a0381e`/`e9905ef`), and a `git subtree pull` dry run
gave 145 unmerged paths and moved our `src/foxglove/` into an upstream crate. So:

- **Don't** `git subtree pull` upstream, and don't depend on its crates. The
  [Pulling upstream](#pulling-upstream) steps below are history, for the old layout only.
- **Do** port upstream fixes one at a time, by hand or by cherry-pick, and say which upstream
  commit each came from (`Foxglove (#NNN, upstream <sha>)`).
- **Do** send generic work back with `git subtree split` ([below](#sending-a-change-upstream)).
- Licence: upstream's crates are GPL-3.0-only like `app/`. Their code may come into `app/`,
  never into our MIT crates (`crates/`).
- The editor profiles were ported as ideas, not code: `app/src/keymap_editor/profile.rs`
  (#579, #582), layers and layouts (#583).

Write-up: `research/keypeek-upstream-579.md` in the workspace folder.

### Upstream fixes checked (to `a12d051`)

| Upstream | What | Here |
|---|---|---|
| [`b52dd3e`](https://github.com/srwi/keypeek/commit/b52dd3e) | missing/incorrect keycodes | Not needed for ZMK: our ZMK lists come from `zmk-studio-api`'s `BehaviorRole::standard_candidates`, which already has scroll left/right, a backlight level and no breathing, and we already have a Special page. The rest (Alt Repeat, Key Lock, `RM_*` names, dynamic macros) is QMK, which is read-only here (#74) and takes its keycodes from `qmk-via-api`. |
| [`2608346`](https://github.com/srwi/keypeek/commit/2608346) | QMK Space Cadet, Auto Shift, Combo keycodes | Not taken: QMK editing is read-only here (#74), and the QMK picker's keycodes come from `qmk-via-api`'s categories, not a hand-made catalog. Revisit with QMK writes. |
| [`b933833`](https://github.com/srwi/keypeek/commit/b933833) | QMK one-shot mods | Not taken: same reason as `2608346` (QMK, read-only, `qmk-via-api` catalog). |
| [`9e061b7`](https://github.com/srwi/keypeek/commit/9e061b7) | reader threads end on drop | Ported (#579): `impl Drop for Keyboard` closes the command channel and wakes a reader that sleeps until a report (ZMK over the link, the Studio fake), so it ends on a silent board. QMK readers already time out. Test: `keyboard::tests::dropping_the_keyboard_ends_a_silent_reader`. |

## What we changed, and keep small

At first only names, so upstream changes merged cleanly:

- `Cargo.toml`: package `foxglove`, version, description, repository, bundle name/identifier
- Window titles, tray tooltip, log prefixes, the Wayland layer namespace
- Settings dir: `ProjectDirs::from("", "", "Foxglove")`. Until Foxglove saves its own,
  `Settings::load` reads KeyPeek's `settings.ini`; KeyPeek's file is never written
- Settings footer: link to this repo plus "based on KeyPeek by Stephan Rumswinkel"
- `cargo-appimage.desktop`, `flake.nix`, `resources/macos-info-plist-ext.xml`: names

New Foxglove code goes in new modules (`src/foxglove/`) rather than into upstream files.
`app/` keeps its own Cargo workspace and `Cargo.lock`, like upstream, so lockfile updates merge too.

## Pulling upstream

Before the hard fork only (upstream's layout up to `d7f5a89`); don't run this against today's
upstream.

```bash
git fetch https://github.com/srwi/keypeek master
git subtree pull --prefix=app https://github.com/srwi/keypeek master
```

Expect conflicts only in the renamed lines above; keep our names and take upstream's logic.
Then build and test (`cargo test --manifest-path app/Cargo.toml`) and check the overlay.

When upstream merges its own monitor picker (srwi/keypeek#49), ours (`monitor-select`) will
conflict with it in `settings.rs`, `ui_settings.rs` and `eframe_host.rs`: take upstream's.

The layer overlay's card (#534) moved KeyPeek's **Display duration** and **Activation delay**
rows to the top of the appearance grid, under a "When it shows" heading
(`settings_ui::grid_heading`), and calls `settings_ui::keymap_choice` before the sections. On a
conflict there, keep upstream's rows and move the two again.

The card's first row is Foxglove's **Show layer overlay** (`overlay_shown::row`, then
`overlay_shown::mute_if_off`), and `overlay_visible` in `settings_sync.rs` goes through
`overlay_shown::visible`. On a conflict there, keep upstream's logic and put the two calls back.

## Sending a change upstream

```bash
git subtree split --prefix=app -b keypeek-export
```

That branch has `app/` at its root with history; cherry-pick the generic commit onto a branch
of BanditF/keypeek, revert our renames in it, and open the PR to srwi/keypeek from there.
