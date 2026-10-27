//! Foxglove: when the layer overlay may take the mouse while Settings is open.
//!
//! With Settings open the overlay takes the mouse over its keys, so it can be dragged (to any
//! display) or a key clicked to open the editor. The rule: the overlay is clickable only while
//! the Settings window has the focus. Minimized, closed, or with another app in front, the
//! overlay is click-through again and a click goes to the game or desktop underneath.
//!
//! Clicking or dragging the overlay moves the focus from Settings to the overlay's own window;
//! that isn't losing it: the overlay stays clickable while it has the focus (or a drag of it
//! is under way), as long as it got there from Settings. Any other window taking the focus
//! ends it, and only Settings getting the focus back starts it again.
//!
//! It changes only on the windows' focus and minimize events (each one repaints the UI); there
//! is no timer.

/// What the windows' focus looks like this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// The Settings window is open.
    pub open: bool,
    /// It has the keyboard focus (it's the foreground window).
    pub settings_focused: bool,
    /// It is minimized.
    pub minimized: bool,
    /// The overlay's own window has the focus (it was clicked).
    pub overlay_focused: bool,
    /// A drag of the overlay is under way.
    pub dragging: bool,
}

/// Whether the overlay takes the mouse after `seen`, given whether it did (`armed`).
pub fn next(armed: bool, seen: Seen) -> bool {
    if !seen.open || seen.minimized {
        return false;
    }
    if seen.settings_focused {
        return true;
    }
    // Focus went from Settings to the overlay (a click or drag on it): keep it. Anything
    // else in front (another app, the desktop): click-through.
    armed && (seen.overlay_focused || seen.dragging)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focused() -> Seen {
        Seen {
            open: true,
            settings_focused: true,
            ..Seen::default()
        }
    }

    fn other_app() -> Seen {
        Seen {
            open: true,
            ..Seen::default()
        }
    }

    #[test]
    fn only_a_focused_settings_window_makes_it_clickable() {
        assert!(next(false, focused()));
        assert!(!next(false, other_app()));
        assert!(!next(true, other_app()), "another app took the focus");
    }

    #[test]
    fn minimized_settings_leaves_it_click_through() {
        // The live bug: Settings minimized, the overlay still took the clicks meant for a game.
        let minimized = Seen {
            minimized: true,
            settings_focused: false,
            ..focused()
        };
        assert!(!next(true, minimized));
        // Even when Windows still calls the minimized window focused.
        assert!(!next(
            true,
            Seen {
                minimized: true,
                ..focused()
            }
        ));
    }

    #[test]
    fn closing_settings_leaves_it_click_through() {
        assert!(!next(true, Seen::default()));
        let closed_mid_drag = Seen {
            dragging: true,
            overlay_focused: true,
            ..Seen::default()
        };
        assert!(!next(true, closed_mid_drag));
    }

    #[test]
    fn clicking_or_dragging_the_overlay_is_not_losing_the_focus() {
        let mut armed = next(false, focused());
        // The click on the overlay moves the focus to its window.
        let clicked = Seen {
            overlay_focused: true,
            ..other_app()
        };
        armed = next(armed, clicked);
        assert!(armed);
        // The drag carries on, even across a frame where neither window says it has the focus.
        armed = next(
            armed,
            Seen {
                dragging: true,
                ..other_app()
            },
        );
        assert!(armed);
        armed = next(
            armed,
            Seen {
                dragging: true,
                ..clicked
            },
        );
        assert!(armed);
        // Dropped; the overlay still has the focus.
        armed = next(armed, clicked);
        assert!(armed);
        // Then the game is clicked: click-through.
        armed = next(armed, other_app());
        assert!(!armed);
    }

    #[test]
    fn the_overlay_focused_without_settings_first_does_not_arm_it() {
        let overlay = Seen {
            overlay_focused: true,
            ..other_app()
        };
        assert!(!next(false, overlay));
        assert!(!next(
            false,
            Seen {
                dragging: true,
                ..overlay
            }
        ));
    }

    #[test]
    fn focus_back_on_settings_makes_it_draggable_again() {
        let mut armed = next(false, focused());
        armed = next(armed, other_app());
        assert!(!armed);
        armed = next(
            armed,
            Seen {
                minimized: true,
                ..other_app()
            },
        );
        assert!(!armed);
        // Restored and focused.
        armed = next(armed, focused());
        assert!(armed);
    }
}
