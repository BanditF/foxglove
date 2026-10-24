# Layer rescue notes

Layer indices stay unchanged; SYSTEM is appended at index 6. Positions below use
QWERTY legends, including when GAME is active.

| Index | Layer | How to reach it |
| --- | --- | --- |
| 0 | QWERTY | Base layer; SYSTEM + A returns here and clears layer locks. |
| 1 | NUMBER | Hold the middle left thumb; SYSTEM + N toggles it. |
| 2 | SYMBOL | Hold the middle right thumb; SYSTEM + S toggles it. |
| 3 | GAME | Backspace + Esc combo, or SYSTEM + G, toggles it. |
| 4 | GAME+ | Hold the middle left thumb in GAME. |
| 5 | FUNCTION | Hold both middle thumbs: NUMBER + SYMBOL or GAME+ + SYMBOL. SYSTEM + F retains the FUNCTION toggle; see the conditional-layer caveat below. |
| 6 | SYSTEM | Keep FUNCTION active, then hold Tab (outer-left top key). Keep the thumbs and Tab held while using it. |

FUNCTION keeps F1-F12 in their previous positions, mouse controls, mute and the
scroll encoder, Print Screen, Scroll Lock and Pause. Caps Lock is on A and Insert
is on Z. The old Bluetooth/output positions in NUMBER and SYMBOL are now `&none`
(disabled), so they cannot fall through to typing keys.

| Moved control | New SYSTEM position |
| --- | --- |
| Bluetooth profiles 0-4 | Q, W, E, R, T (top row); profile 4 is newly available. |
| Clear current Bluetooth profile | Left Ctrl (far left of bottom letter row). |
| Clear all Bluetooth profiles | Esc (far right of bottom letter row). |
| USB / Bluetooth output | Y / U (top row). |
| Return to QWERTY (`&to 0`) | A. |
| Toggle NUMBER / SYMBOL / GAME / FUNCTION | N / S / G / F. |
| Left-half bootloader | C. |
| Right-half bootloader | Comma or slash (the old H binding moved to slash, off the home row). |
| Left-half / right-half reset | Z / period. |
| Soft off, both halves | M, hold for 2 seconds. Use the physical reset buttons to wake. |

SYSTEM's Tab is transparent so the held FUNCTION key remains its entry point.
The middle thumbs are transparent too, preserving their momentary releases.
The requested FUNCTION toggle remains `&tog 5`, as before. Its interaction with
conditional activation still needs a hardware check. Use held thumbs for rescue.
