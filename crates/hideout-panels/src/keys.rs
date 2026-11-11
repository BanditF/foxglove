// SPDX-License-Identifier: MIT
//! Key capture for the Keys section (§4.7, #991): a [`Chord`] is the key a user binds to a
//! module's action with **Set key**. It is kept and shown as people read it ("Ctrl+Alt+M",
//! "F13"); the hub registers it as a system-wide key (the shell's hotkeys) and a press sends
//! `action.invoke {source: "key"}`.
//!
//! The rule: F13 to F24 alone (keys a keyboard's macro layer sends), or any other key with
//! Ctrl or Alt held, so a bound key never takes a letter or a plain function key from typing.
//! Esc cancels the capture. A few chords Windows keeps for itself are refused.

use egui::{Key, Modifiers};

/// A bindable key with its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

/// What one key press during a capture comes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Press {
    /// A key to bind.
    Bind(Chord),
    /// Esc: the capture ends, nothing changes.
    Cancel,
    /// Not a key to bind; the capture goes on and shows [`NEEDS_MODIFIER`] or [`RESERVED`].
    Refused(&'static str),
    /// A modifier alone (Ctrl going down): wait for the key.
    Wait,
}

/// The Keys section's words. "Set key" is the plan's (docs/hideout-plugin-modules-ux.md);
/// the others are proposed (designer to confirm).
pub const SET_KEY: &str = "Set key";
pub const PRESS_A_KEY: &str = "Press a key\u{2026}";
pub const CANCEL: &str = "Cancel";
pub const CHANGE: &str = "Change";
pub const REMOVE: &str = "Remove";
pub const NEEDS_MODIFIER: &str = "Use F13 to F24, or hold Ctrl or Alt with the key.";
pub const RESERVED: &str = "Windows keeps that key for itself.";

/// "<Key> already runs <Action> in <Module>." (proposed).
pub fn taken_here(chord: &str, action: &str, module: &str) -> String {
    format!("{chord} already runs {action} in {module}.")
}

/// "Another app uses <Key>." (proposed): the system refused to register it.
pub fn taken_elsewhere(chord: &str) -> String {
    format!("Another app uses {chord}.")
}

const F13_TO_F24: [Key; 12] = [
    Key::F13,
    Key::F14,
    Key::F15,
    Key::F16,
    Key::F17,
    Key::F18,
    Key::F19,
    Key::F20,
    Key::F21,
    Key::F22,
    Key::F23,
    Key::F24,
];

impl Chord {
    /// One key press while capturing (`key` the physical key when egui knows it).
    pub fn press(key: Key, m: Modifiers) -> Press {
        if key == Key::Escape && !m.ctrl && !m.alt && !m.shift {
            return Press::Cancel;
        }
        if vk(key).is_none() {
            return Press::Wait;
        }
        let c = Chord {
            ctrl: m.ctrl || m.command,
            alt: m.alt,
            shift: m.shift,
            key,
        };
        if c.reserved() {
            return Press::Refused(RESERVED);
        }
        if F13_TO_F24.contains(&key) || c.ctrl || c.alt {
            Press::Bind(c)
        } else {
            Press::Refused(NEEDS_MODIFIER)
        }
    }

    /// Chords Windows answers itself (task switching, the secure attention keys, closing a
    /// window, the Start menu).
    fn reserved(&self) -> bool {
        let only = |ctrl: bool, alt: bool| self.ctrl == ctrl && self.alt == alt;
        matches!(
            (self.key, self.ctrl, self.alt),
            (Key::Delete, true, true) | (Key::Escape, true, _) | (Key::Escape, _, true)
        ) || (self.key == Key::Tab && self.alt)
            || (self.key == Key::F4 && only(false, true))
    }

    /// Parses what [`Chord`]'s `Display` writes ("Ctrl+Alt+M"); `None` for anything else.
    pub fn parse(text: &str) -> Option<Chord> {
        let mut c = Chord {
            ctrl: false,
            alt: false,
            shift: false,
            key: Key::Escape,
        };
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let key = Key::from_name(parts.pop()?)?;
        for p in parts {
            match p {
                "Ctrl" if !c.ctrl => c.ctrl = true,
                "Alt" if !c.alt => c.alt = true,
                "Shift" if !c.shift => c.shift = true,
                _ => return None,
            }
        }
        vk(key)?;
        c.key = key;
        Some(c)
    }

    /// The Win32 `RegisterHotKey` modifier flags (`MOD_ALT` 1, `MOD_CONTROL` 2, `MOD_SHIFT` 4).
    pub fn win_modifiers(&self) -> u32 {
        u32::from(self.alt) | (u32::from(self.ctrl) << 1) | (u32::from(self.shift) << 2)
    }

    /// The Win32 virtual-key code.
    pub fn win_vk(&self) -> u32 {
        vk(self.key).unwrap_or(0)
    }
}

impl std::fmt::Display for Chord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(self.key.name())
    }
}

/// The Win32 virtual-key code of each key that may be bound (winuser.h `VK_*`).
fn vk(key: Key) -> Option<u32> {
    use Key::*;
    let letters = [
        A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    ];
    let digits = [Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9];
    let fs = [
        F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18, F19, F20,
        F21, F22, F23, F24,
    ];
    if let Some(i) = letters.iter().position(|k| *k == key) {
        return Some(0x41 + i as u32);
    }
    if let Some(i) = digits.iter().position(|k| *k == key) {
        return Some(0x30 + i as u32);
    }
    if let Some(i) = fs.iter().position(|k| *k == key) {
        return Some(0x70 + i as u32);
    }
    Some(match key {
        Escape => 0x1B,
        Tab => 0x09,
        Backspace => 0x08,
        Enter => 0x0D,
        Space => 0x20,
        PageUp => 0x21,
        PageDown => 0x22,
        End => 0x23,
        Home => 0x24,
        ArrowLeft => 0x25,
        ArrowUp => 0x26,
        ArrowRight => 0x27,
        ArrowDown => 0x28,
        Insert => 0x2D,
        Delete => 0x2E,
        Semicolon => 0xBA,
        Equals | Plus => 0xBB,
        Comma => 0xBC,
        Minus => 0xBD,
        Period => 0xBE,
        Slash => 0xBF,
        Backtick => 0xC0,
        OpenBracket => 0xDB,
        Backslash => 0xDC,
        CloseBracket => 0xDD,
        Quote => 0xDE,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, alt: bool, shift: bool) -> Modifiers {
        Modifiers {
            ctrl,
            alt,
            shift,
            command: ctrl,
            mac_cmd: false,
        }
    }

    #[test]
    fn f13_to_f24_bind_alone_and_other_keys_need_ctrl_or_alt() {
        let none = mods(false, false, false);
        assert!(matches!(Chord::press(Key::F13, none), Press::Bind(_)));
        assert!(matches!(Chord::press(Key::F24, none), Press::Bind(_)));
        assert_eq!(Chord::press(Key::M, none), Press::Refused(NEEDS_MODIFIER));
        assert_eq!(
            Chord::press(Key::F5, mods(false, false, true)),
            Press::Refused(NEEDS_MODIFIER)
        );
        let Press::Bind(c) = Chord::press(Key::M, mods(true, true, false)) else {
            panic!("Ctrl+Alt+M binds");
        };
        assert_eq!(c.to_string(), "Ctrl+Alt+M");
        assert_eq!((c.win_modifiers(), c.win_vk()), (3, 0x4D));
        assert_eq!(Chord::press(Key::Escape, none), Press::Cancel);
        assert_eq!(
            Chord::press(Key::Copy, mods(true, false, false)),
            Press::Wait
        );
    }

    #[test]
    fn windows_own_chords_are_refused() {
        for (k, m) in [
            (Key::Delete, mods(true, true, false)),
            (Key::Tab, mods(false, true, false)),
            (Key::F4, mods(false, true, false)),
            (Key::Escape, mods(true, false, false)),
        ] {
            assert_eq!(Chord::press(k, m), Press::Refused(RESERVED), "{k:?}");
        }
        assert!(matches!(
            Chord::press(Key::F4, mods(true, true, false)),
            Press::Bind(_)
        ));
    }

    #[test]
    fn chords_round_trip_as_people_read_them() {
        for text in ["F13", "Ctrl+Alt+M", "Alt+Shift+1", "Ctrl+Down", "Ctrl+F24"] {
            let c = Chord::parse(text).unwrap();
            assert_eq!(c.to_string(), text);
        }
        assert_eq!(Chord::parse("F13").unwrap().win_vk(), 0x7C);
        assert_eq!(Chord::parse("Ctrl+Down").unwrap().win_vk(), 0x28);
        for bad in ["", "Ctrl+", "Hyper+M", "Ctrl+Ctrl+M", "Ctrl+Copy", "m+Ctrl"] {
            assert_eq!(Chord::parse(bad), None, "{bad:?}");
        }
    }
}
