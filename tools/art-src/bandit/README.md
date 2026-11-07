# Bandit art generators

These are the versioned sources of the owner's Bandit fox art. They are © the repo owner, all
rights reserved, and not covered by this repo's code licences (see `LICENSING.md` and
`docs/art-licensing.md` F5). They need Python 3 and Pillow, and take no inputs.

- `bandit_art.py`: the first Bandit library (emblem, idle, radar, signal, skyline and the
  rest). `Bandit_Emblem` from it is the firmware's one built-in.
- `motion/bandit_motion.py`: the Bandit Motion set bundled in `app/resources/art/bandit-motion/`.
  It imports its helpers from `../bandit_art.py`.
- `motion/check.py`: the photosensitivity check. No step of the play order may change more than
  4% of the screen or flip half of any 8x8 block. `scripts/test_bandit_art.py` runs it on the
  shipped blobs and on a fresh generator run (it's part of check.ps1's Python tests).

The working copy, with its frame folders, GIF previews and review sheets, is
`C:\Users\fuchs\splitkeeb\art-src\bandit`. Only the sources live here. Keep the two in step.

## Regenerate the bundled set

From the repo root:

```
python -I tools/art-src/bandit/motion/bandit_motion.py OUT
python -I tools/art-src/bandit/motion/check.py OUT
python -I scripts/flipper_art.py app/resources/art/bandit-motion/bandit-NAME.bin "OUT/Bandit_NAME:polarity=as-is:label=Bandit NAME"
```

Frame folders are named `Bandit_NAME` (for example `Bandit_signal_v2`), and the blob uses the
same name with dashes (`bandit-signal-v2.bin`). Frames are saved with lit pixels black, so use
`polarity=as-is`. The blobs were made with Pillow 13. Another Pillow version can rasterise a
pixel differently, so after a regenerate, compare the result before you commit it.

The first library: `python -I tools/art-src/bandit/bandit_art.py OUT [Bandit_name ...]`.
