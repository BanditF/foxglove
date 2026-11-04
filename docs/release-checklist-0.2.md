# Release checklist: Foxglove 0.2.0

App `app-v0.2.0` and firmware `fw-v0.2.0`, from one candidate commit on main. The release
agent fills in the evidence column; On-call runs the hardware rows; the user gives the go.
Nothing here is tagged or published until the last section, and that needs the user's go in
chat for the exact commit, artifacts and notes.

The notes are [release-notes/0.2.md](release-notes/0.2.md). The rollback runbook is RESCUE.md
(in `C:\Users\fuchs\splitkeeb`, outside the repo; read it each time, it changes with every
flash). The 0.1 checklist is at the end of [release-notes/0.1.md](release-notes/0.1.md).

Status: **prepared, not started.** Candidate: not chosen yet (main 37953e9 or later, after the
PENDING items).

## 0. Open work before a candidate

Each item lands or is dropped, and its PENDING line in the notes is resolved either way.

- [ ] BanditF/zmk-raw-hid#2 (foxglove#680): Raw HID report queue. If it lands, the firmware's
      west manifest points at the merged commit (not a branch), and #682's CI-only firmware
      build is closed. Health counters land first. *Merged into `nonblocking` as 2a26c3d and
      #682 closed; the manifest still names the branch, and hardware is under Known untested
      items.*
- [x] #678: the lab's red cases on main, done by #689 (2bc521c): they report BLOCKED with the
      outside cause; a product failure is still a FAIL.
- [x] #664: the right's bootloader drive after standby, done by #687 (d9fa1e2): one replug prompt.
- [x] The automated sitting: #690 (f7564981), `scripts/lab/sitting.ps1`, 0 user steps. Its run on the candidate is in section 4.
- [x] The keyboard's self-report and `foxglove-lab health` (the test after a batch flash), done by #711.
- [x] The Dynamic Lighting stack (#691: #693, #697, #699, #700, #704) lands, and its PENDING note is written.
- [ ] #681's `lights_page.py` step passes in a release gate on the candidate (#677).

## 1. Versions

| Where | Value | Check |
|---|---|---|
| `app/Cargo.toml` and every workspace crate except the plugins | 0.2.0 | [ ] |
| `boards/shields/nice_view_custom/FIRMWARE_VERSION` | 0.2.0 | [ ] |
| `crates/plugin-api` `API_VERSION`, `docs/local-api.md` Versioning | 1.10 | [ ] |
| Bundled plugins (`crates/plugin-*/Cargo.toml`, `plugin.toml`) | their own (1.0.0) | [ ] |
| `Cargo.lock` and `app/Cargo.lock` agree with the manifests (`--locked` builds) | | [ ] |
| MSI version (cargo-wix from `app/Cargo.toml`, `app.yml` from the tag) above 0.1.x | 0.2.0 | [ ] |
| The candidate's BUILD_INFO (D7 09) on both halves | 0.2.0, profile, SHA | [ ] |

The app and firmware agree by HELLO capability bits, not by version, so no app check needs a
firmware version number. **Update firmware…** compares versions only to ask before a
downgrade: rolling back to a 0.1.0 build asks the user to allow it. `foxglove_flash.py` checks
the half marker and reads the build back; it doesn't compare versions.

## 2. Gates on the candidate commit

Record the commit, run ids and results here.

| Check | How | Evidence |
|---|---|---|
| CI: Script checks (`checks.yml`) green | on the PR / push | |
| CI: Build ZMK firmware (`build.yml`) green | on the push; preflight and the GA hook check pass | |
| CI: ZMK simulator (`zmk-sim.yml`) green, including `relay-sim` and `tests/zmk-sim/hostlink-ga` | on the push | |
| CI: Rust crates (`rust.yml`) green | `gh workflow run rust.yml --ref <branch>` (manual) | |
| CI: App (`app.yml`) green, Windows and Linux | `gh workflow run app.yml --ref <branch>` (manual, no publish) | |
| `scripts/check.ps1` green | `powershell -ExecutionPolicy Bypass -File scripts/check.ps1` | |
| Code-reviewer gate on every PR since the last candidate | the PRs' reviews | |
| Notes under GitHub's 125,000-character release body limit | `(Get-Content docs/release-notes/0.2.md -Raw).Length` | |

## 3. Firmware artifacts

From the candidate's `build.yml` run: the artifact `foxglove-fw-v0.2.0-<sha>` (four UF2s and
`settings-reset`) and `foxglove-0.2.0-release-<sha>` (the release assets).

| Check | Evidence |
|---|---|
| `foxglove-fw-v0.2.0-<sha>-left-dev.uf2`, `-right-dev.uf2` | |
| `foxglove-fw-v0.2.0-<sha>-left-ga.uf2`, `-right-ga.uf2` | |
| `foxglove-0.2.0-left.uf2`, `-right.uf2` (GA), `-settings-reset.uf2`, `SHA256SUMS` covering them | |
| Half markers say VER 0.2.0, the right profile and the candidate's SHA (CI preflight) | |
| GA builds carry no test hook; dev builds carry them all (`check_hooks.py`) | |
| `python scripts/flash/foxglove_flash.py --left L --right R --dry-run` on the downloaded GA pair, and on the dev pair | |
| SHA256SUMS matches the downloads (`sha256sum -c SHA256SUMS`) | |

### Flash headroom

The bootloader's limit is 0xAD000. Record each image's end and headroom from the candidate's
"Preflight the UF2s" step, and compare with the last main build.

| Image | Last main build (9f29911, run 40992332722, still named 0.1.0) | Candidate |
|---|---|---|
| left dev | end 0x84A00, 161.5 KiB | |
| right dev | end 0x6B900, 261.8 KiB | |
| left GA | end 0x80900, 177.8 KiB | |
| right GA | end 0x68000, 276.0 KiB | |

A drop of more than a few KiB against the last main build needs a reason in the notes.

## 4. Release gate and sitting (On-call, hardware)

The user's go for each flash; the release gate never flashes.

- [ ] On-call flashes the candidate's **dev** pair (left first, then right; RESCUE.md).
- [ ] `powershell -File scripts\release-test.ps1 -Firmware <candidate firmware folder> -NonInteractive -AllowApp`
      with `FOXGLOVE_APP_OK=1`: verdict **PASS**, zero FAIL, zero known-red cases, and no
      BLOCKED release case. `report.json` is schema `foxglove-lab-report` version 2, and
      `firmware.left` / `firmware.right` show the candidate's SHA and profile on both halves.
- [ ] The 30-minute soak (`scripts/lab/soak.py`) on the same build.
- [ ] The automated sitting (`scripts/lab/sitting.ps1`, #690) on the same build, its report linked here.
- [ ] `foxglove-lab health` on both halves after the batch flash (once it lands).
- [ ] The human-only steps that remain (the needs-eyes list from
      `python scripts/test_sitting_parity.py --list`, through the #90 queue). Steps that passed
      before are not repeated unless their code changed since.
- [ ] **GA:** with the user's go, On-call flashes the candidate's **GA** pair and runs
      `foxglove-lab run --suite ga`: PASS on every `ga` case (BUILD_INFO profile `ga` on both
      halves, HELLO bits 5 and 9 clear, D7 05 and D7 08 answer 4, INVOKE refusals of
      &bootloader / &bt / &out / &soft_off answer 6). Then the dev pair goes back, unless the
      user keeps GA.
- [ ] Links to `report.html` and `junit.xml` of each run go into the notes.

## 5. Rollback path (RESCUE.md)

- [ ] The known-good build RESCUE.md names is on this PC, both halves, and passes
      `foxglove_flash.py --firmware <folder> --half both --dry-run`.
- [ ] One rollback rehearsal on hardware (On-call, the user's go): flash the candidate, then the
      known-good build back with `foxglove_flash.py`, typing over USB after each half. Note that
      the known-good build reports 0.1.0, so **Update firmware…** would ask to allow the
      downgrade; the flasher doesn't.
- [ ] `--resume` recovers a half left in its bootloader (#654), dry run at least.
- [ ] RESCUE.md names the 0.2.0 GA build as known-good after the release.

## 6. MSI and app

- [ ] The app workflow's manual run on the candidate: `foxglove-windows` artifact with the exe
      and `foxglove-x64.msi` (WiX 3.14, cargo-wix 0.3.8 `--locked`), version 0.2.0.
- [ ] On a Windows test account (#90 row L, never the user's desktop): per-user install with no
      UAC, the Start menu entry, start at sign-in, an upgrade from an older test build
      (`app.yml`'s `test_version`), downgrade refused, the 0.1 per-machine guard message,
      uninstall removing the shortcut and the Run value and keeping `%APPDATA%\Foxglove\config`.
- [ ] Automatic updates: the build still carries the placeholder `app/resources/update-key.pub`,
      so the app says updates are off. Either that stays (the notes say so) or the real key is
      in and `FOXGLOVE_UPDATE_KEY` is set as a repository secret: `app.yml`'s publish job refuses
      to run without it.
- [ ] Credits: KeyPeek's GPL-3.0 notice and source, the crates' MIT notices, the bundled fonts'
      SIL OFL, new dependency notices (`cargo deny` green).

## 7. Notes final

- [ ] No PENDING line left in `docs/release-notes/0.2.md`: each became a note or a known issue.
- [ ] The header says the final candidate commit, not "up to #703 at 37953e9".
- [ ] PRs merged after 37953e9 are in the notes (`gh pr list --state merged --base main --limit 300`).
- [ ] The "Known untested items" boxes are ticked with the revision they passed on, or
      moved to issues.
- [ ] The release-gate, soak and sitting reports are linked.

## 8. The user's go

- [ ] The Hub asks the user, with: the version (0.2.0), the candidate commit, the artifact list
      with SHA256SUMS, the notes, this checklist with every row filled, and what wasn't tested.
- [ ] The user's go is in chat, for that commit and those artifacts. Anything that changes
      afterwards needs a new go.

## 9. Tag and publish (written down, not done)

Done only after section 8. A tag push publishes by itself, so nobody pushes a tag by hand
before the go.

Firmware:
1. Create a **draft** GitHub release "Firmware fw-v0.2.0" targeting the candidate commit, with
   the four files of `foxglove-0.2.0-release-<sha>` and the notes from
   `docs/release-notes/0.2.md`.
2. Check the draft's target is still the candidate (re-point or rebuild if main moved in
   firmware since).
3. Publish the draft: that creates the tag `fw-v0.2.0`. Its `build.yml` run checks the tag
   against FIRMWARE_VERSION and finds the release there, so it uploads nothing.
4. Download the release's files, `sha256sum -c SHA256SUMS`, and run
   `foxglove_flash.py --left foxglove-0.2.0-left.uf2 --right foxglove-0.2.0-right.uf2 --dry-run`.

App:
5. Push the tag `app-v0.2.0` at the candidate commit. `app.yml` builds the exe and MSI at that
   version, checks `FOXGLOVE_UPDATE_KEY`, creates the release with generated notes, signs the
   update manifest and uploads the assets.
6. Replace the generated notes with the app part of `docs/release-notes/0.2.md` if they differ.
7. Install the published MSI on the test account once (section 6's smoke test, from the release).

After:
8. RESCUE.md: the 0.2.0 GA build becomes the known-good rollback.
9. `release/0.2` branch from the tag commit for 0.2.x fixes, if any are expected.
10. Bump main to 0.3.0 (app, crates, FIRMWARE_VERSION) in its own PR.
