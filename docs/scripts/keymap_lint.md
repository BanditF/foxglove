# keymap_lint.py — offline checks for the 48-key Corne keymap

`scripts/keymap/keymap_lint.py` runs cheap offline checks on the 48-key Corne keymap with no
ZMK build. It reads the keymap source and models momentary thumb layers, not ZMK timing or
arbitrary behaviors. Its checks: every layer has exactly 48 bindings; every layer reference
(`&mo`, `&tog`, `&to`, `&lt`, `&sl`, and the `conditional_layers` rules) names a layer within
0..N-1; `&soft_off` sits behind a hold, stays out of the vendor Q+S+Z combo, and appears only
in layer bindings; and replaying every press/release ordering of the two thumb keys
(positions 43 and 46) over base layers 0 and GAME leaves no stuck layer.

## How to run

Exactly one argument, the keymap file:

    python scripts/keymap/keymap_lint.py config/eyelash_corne.keymap

## Flags

None. The script takes no flags; with anything but exactly one argument it prints the usage
line (`Usage: keymap_lint.py config/eyelash_corne.keymap`) and exits 1.

## Exit codes

0: all checks passed. 1: one or more check failures, or the wrong number of arguments.

## What it reads and writes

Reads: the keymap file given as the argument — the `keymap` node with each layer's
`display-name` and `bindings`, `status = "reserved"` layers (which must come after every real
layer), the `conditional_layers` rules (`if-layers` / `then-layer`), and the `combos` node's
`key-positions`.

Writes: to stdout only — one `FAIL: ...` line per failure, or the single pass line
`Keymap passed: 48 bindings/layer, layer references, thumb replay, and soft-off safety.`

## Example

    python scripts/keymap/keymap_lint.py config/eyelash_corne.keymap

checks the checked-in Corne keymap; it prints the pass line and exits 0, or one `FAIL` line
per problem and exits 1.
