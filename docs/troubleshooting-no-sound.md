# Troubleshooting: no sound in app X

Master volume (Knob card, per device) turns each app's volume down in Windows while the
knob is on master. If an app is silent or very quiet, it may have been left scaled down.

## 1. Read the log

The log is `%APPDATA%\Foxglove\logs\foxglove.log`. Search for `master volume`.

- `engaged for <device>: N sessions` is followed by one line per app:
  `vivaldi.exe: own 100 %, applied 63 %, Windows 63 %`.
  - **own**: the level the app has when master volume is off. This is the one to trust.
  - **applied**: what Foxglove last wrote (own times master).
  - **Windows**: what Windows reports for the app right now.
- `disengaged for <device>: N sessions, K write failures` has the same lines. After a
  clean stop, Windows should equal own. A `WRITE FAILED` line names an app that could not
  be put back.
- `left scaled: <app> ...`: the app was found below its own level, and the journal explains
  it (Windows still has the applied level). Foxglove puts it back to its own level.
- `left scaled? <app> ...`: the app is below its own level but nothing in the journal
  explains it. Foxglove does not guess and does not touch it. Raise it by hand.
- `not started for <device>`: engaging failed; apps were put back where they were.

The Health view shows the last summary as "Knob - Master volume apps"; it warns when a
write failed.

## 2. Check an app's own and applied level

The journal is `master-volume-journal.json` in the Foxglove config folder
(`%APPDATA%\Foxglove`). Under `apps`, each app (by session id) has `own` and `applied`
(0.0 to 1.0). While the knob is on master, `applied` should be `own` times the master
factor. With master volume off, `applied` should equal `own`, or the entry should be gone.
Do not edit the file while Foxglove runs.

## 3. Safe manual fix

1. Open the Knob card and turn master volume **off for that device**. Foxglove sets the
   device back to master's level and every app back to its own level.
2. If the app is still quiet, check its slider in the Windows Volume mixer. If the log has
   `left scaled?` for it, that level is the one to set by hand.
3. Turn master volume on again only after the log shows `disengaged ... 0 write failures`.

If the log says the journal can't be read, master volume waits. Close apps that may hold
the file, then restart Foxglove. The file is kept and never written over.
