<!-- Say what changed and why, and which issue it is for ("For #NNN" / "Closes #NNN"). -->

## Checklist
- [ ] **Tests:** window-free tests for the new logic (no foxglove.exe launch, no keyboard); hardware gaps named below.
- [ ] **Polling:** one line, "Polling: none", or the event that starts and stops each wait.
- [ ] **Docs:** user-visible changes in `docs/release-notes/<next>.md`; protocol changes in `host_link.h` and Foxglove link together.
- [ ] **UI:** an off-screen PNG (`FOXGLOVE_UI_PREVIEW_DIR`, copied to `review\`) for any visible change.
- [ ] **Gate:** `scripts/check.ps1` passed once before this PR (again only after code changes).

Not covered (hardware, live runs):

🤖 Generated with [Claude Code](https://claude.com/claude-code)
