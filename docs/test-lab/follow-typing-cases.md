# Lab cases: follow the keyboard I type on (#802)

App › Keyboard shown › "Follow the keyboard I type on" moves the focus to the keyboard that last
typed: after 6 Raw Input events in a row from it (about 3 keys), each within 2 s of the last, at
most once a second. Code: `app/src/foxglove/follow_typing.rs`. The window-free tests feed synthetic
device events; these cases prove it on real keyboards.

**Status: queued for the next batch. Not run yet.** The typing has to be done by a person: the lab
never types (its safety rules reject typing behaviors), so the typing steps are `HUMAN` rows for
the needs-human queue (#90). The agent steps only read: the local API (`focus.get`), the `focus.*`
facts and the app log.

## Setup

- Two physical keyboards connected at once: the Eyelash Corne (host link) and the K5 Pro (VIA, USB
  or Bluetooth). Both are listed by `focus.get`, both `connected: true`.
- The user's Foxglove running on a build with #802. The lab doesn't launch it (On-call does, if a
  restart is needed).
- Before: note `focus.get`'s `follow_typing` and `keyboard`, to put them back after.
- A text field to type into that doesn't matter (Notepad). What is typed is never read by
  Foxglove or the lab.

## Cases

| Row | Who | Does | Pass |
|---|---|---|---|
| Off by default | agent | `focus.get` on a fresh `[focus]` (temp `FOXGLOVE_CONFIG_DIR`) | `follow_typing` false |
| Off follows nothing | HUMAN | with the setting off, type a sentence on the K5, then on the Corne | `focus.keyboard` unchanged; no `(typed on)` line in the log |
| Turn on | HUMAN | App › Keyboard shown › tick "Follow the keyboard I type on" | `focus.get` `follow_typing` true; `[focus] follow_typing = true` |
| K5 takes the focus | HUMAN | type a few words on the K5 | within a second of the third key `focus.keyboard` is the K5's key; log `keyboard focus: Eyelash Corne → K5 Pro (typed on)` |
| Corne takes it back | HUMAN | type a few words on the Corne | `focus.keyboard` is the Corne's; one more `(typed on)` line |
| Two at once, no flicker | HUMAN | for 10 s, alternate single keys between the two boards as fast as possible | at most one `focus.keyboard` change per second in the `focus.*` facts; usually none |
| One stray key | HUMAN | one key on the K5 every 3 s, five times | no change |
| A pick holds | HUMAN | while typing on the Corne, pick the K5 in the switcher, keep typing on the Corne | focus stays the K5; then a few words on the K5 and back on the Corne: it follows the Corne again |
| Typing in Foxglove | HUMAN | with it on, type into a Foxglove text field (Plugins › an address field, Screens › a word) with both boards, IME on if available | every character arrives; IME composition commits |
| Turn off | HUMAN | untick it | `follow_typing` false; typing on either board moves nothing; log has no `handing back` error |
| Put back | agent | `focus.set` the noted keyboard; the noted setting | as before the run |

Only keyboard names, focus keys and times go into the report. Never record what was typed.
