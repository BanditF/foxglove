// SPDX-License-Identifier: MIT
//! What is left of the tab strip now that the launcher rail switches pages: the page
//! [`Action`]s the window applies, the keyboard shortcuts, and a panel's [`Look`].

use crate::tabs::{Look, TabState};
use eframe::egui::{self, Key, Modifiers, Pos2, Vec2};
use hideout_panels::{HostedPanel, PanelState};

/// Pop out's tooltip (the header's button).
pub const POPOUT_TIP: &str = "Pop out (Ctrl+Shift+O)";

/// What the user did in the strip, by strip position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Activate(usize),
    Close(usize),
    CloseOthers(usize),
    CloseRight(usize),
    Move {
        from: usize,
        to: usize,
    },
    PopOut(usize),
    /// The tab was dragged out of Hideout's window and let go there (#1054.4): pop it out at
    /// the pointer.
    DockBack(usize),
    Reopen,
}

/// The open arc of the "working on it" ring round `centre`: the tab strip and the sidebar draw
/// the same shape (docs/hideout-supervisor-ux.md, S and T).
pub fn ring_points(centre: Pos2, radius: f32) -> Vec<Pos2> {
    (0..=24)
        .map(|k| {
            let a = 0.6 + k as f32 * (5.2 / 24.0);
            centre + Vec2::angled(a) * radius
        })
        .collect()
}

/// A module panel's look from the panel host: its title, its state in §7.8 words, its badge.
pub fn panel_look(p: &HostedPanel, popped: bool) -> Look {
    use crate::tabs::Mark;
    use hideout_panels::Badge;
    Look {
        title: p.title.clone(),
        icon: hideout_panels::tray::icon_glyph(&p.icon),
        state: panel_state(&p.state),
        mark: match (p.badge, p.badge_count) {
            (Badge::Attention, _) => Mark::Attention,
            (Badge::Dirty, _) => Mark::Dirty,
            (Badge::Count, Some(n)) => Mark::Count(n),
            _ => Mark::None,
        },
        popped,
    }
}

/// The strip's state for a panel state (§7.8 words, never ids).
pub fn panel_state(s: &PanelState) -> TabState {
    match s {
        PanelState::Live => TabState::Live,
        PanelState::Greyed(id) => match id.as_str() {
            "starting" | "updating" | "restarting" => {
                TabState::Busy(hideout_panels::state_words(id).into())
            }
            "off" => TabState::Off,
            other => TabState::Stopped(hideout_panels::state_words(other).into()),
        },
        PanelState::Rejected(_) | PanelState::Broken => {
            TabState::Stopped("Couldn't be shown".into())
        }
        PanelState::Quarantined => TabState::Stopped("Paused after repeated hub restarts".into()),
    }
}

/// A strip shortcut (the window consumes the keys, so the page never sees them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Next,
    Prev,
    Number(u8),
    Close,
    Reopen,
    MoveLeft,
    MoveRight,
    /// Ctrl+Shift+O: pop the active tab out, or dock it back when it is out.
    PopOut,
}

/// Takes this frame's strip shortcuts out of the input: Ctrl+Tab / Ctrl+Shift+Tab and
/// Ctrl+PageDown / Ctrl+PageUp switch in strip order, Ctrl+1–9, Ctrl+W, Ctrl+Shift+T, and
/// Ctrl+Shift+PageUp / PageDown move the active tab. Shifted keys are read first: egui's match
/// ignores an extra Shift.
pub fn shortcuts(ctx: &egui::Context) -> Vec<Shortcut> {
    let cs = Modifiers::COMMAND | Modifiers::SHIFT;
    ctx.input_mut(|i| {
        let mut out = Vec::new();
        let mut take = |m: Modifiers, k: Key, s: Shortcut| {
            for _ in 0..i.count_and_consume_key(m, k) {
                out.push(s);
            }
        };
        take(cs, Key::Tab, Shortcut::Prev);
        take(cs, Key::PageUp, Shortcut::MoveLeft);
        take(cs, Key::PageDown, Shortcut::MoveRight);
        take(cs, Key::T, Shortcut::Reopen);
        take(cs, Key::O, Shortcut::PopOut);
        take(Modifiers::COMMAND, Key::Tab, Shortcut::Next);
        take(Modifiers::COMMAND, Key::PageDown, Shortcut::Next);
        take(Modifiers::COMMAND, Key::PageUp, Shortcut::Prev);
        take(Modifiers::COMMAND, Key::W, Shortcut::Close);
        let nums = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        for (n, k) in nums.into_iter().enumerate() {
            take(Modifiers::COMMAND, k, Shortcut::Number(n as u8 + 1));
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_states_read_in_words() {
        assert_eq!(
            panel_state(&PanelState::Greyed("updating".into())),
            TabState::Busy("Updating".into())
        );
        assert_eq!(
            panel_state(&PanelState::Greyed("crash-stopped".into())),
            TabState::Stopped("Stopped after 5 crashes".into())
        );
        assert_eq!(
            panel_state(&PanelState::Greyed("off".into())),
            TabState::Off
        );
        assert!(matches!(
            panel_state(&PanelState::Rejected("ui.limits".into())),
            TabState::Stopped(w) if !w.contains('.')
        ));
    }
}
