// SPDX-License-Identifier: MIT
//! Tells the router whether Hideout starts at sign-in, so it can publish the read-only fact
//! `module.<id>.starts_at_sign_in` to running modules (sign-in hand-over slice 2). It only
//! watches the Start with Windows switch: it turns nothing on and writes nothing to the system.

use hideout_shell::startup_switch::{Click, Row, StartupControl};

/// Wraps the Settings page's switch and reports "Hideout starts at sign-in" (the switch is
/// ready and on) to `announce` at first and after every change, never twice for the same value.
pub struct Announcing<S> {
    inner: S,
    announce: Box<dyn Fn(bool) + Send>,
    last: Option<bool>,
}

impl<S: StartupControl> Announcing<S> {
    pub fn new(inner: S, announce: Box<dyn Fn(bool) + Send>) -> Self {
        let mut a = Self {
            inner,
            announce,
            last: None,
        };
        let row = a.inner.row();
        a.note(&row);
        a
    }

    fn note(&mut self, row: &Row) {
        // `turn_off` is offered only when the switch is installed here and on.
        let on = row.turn_off;
        if self.last != Some(on) {
            self.last = Some(on);
            (self.announce)(on);
        }
    }
}

impl<S: StartupControl> StartupControl for Announcing<S> {
    fn row(&mut self) -> Row {
        let row = self.inner.row();
        self.note(&row);
        row
    }

    fn click(&mut self, c: Click) {
        self.inner.click(c);
        let row = self.inner.row();
        self.note(&row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Fake {
        on: bool,
    }

    impl StartupControl for Fake {
        fn row(&mut self) -> Row {
            Row {
                turn_off: self.on,
                ..Row::default()
            }
        }
        fn click(&mut self, c: Click) {
            match c {
                Click::Ask => self.on = true,
                Click::TurnOff | Click::Remove => self.on = false,
                _ => {}
            }
        }
    }

    fn rig(on: bool) -> (Announcing<Fake>, Arc<Mutex<Vec<bool>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let a = Announcing::new(Fake { on }, Box::new(move |v| s.lock().unwrap().push(v)));
        (a, seen)
    }

    #[test]
    fn the_first_value_is_announced_at_start() {
        assert_eq!(*rig(false).1.lock().unwrap(), [false]);
        assert_eq!(*rig(true).1.lock().unwrap(), [true]);
    }

    #[test]
    fn only_changes_are_announced() {
        let (mut a, seen) = rig(false);
        for _ in 0..3 {
            a.row();
        }
        a.click(Click::Cancel);
        assert_eq!(*seen.lock().unwrap(), [false]);
        // The fake turns on at Ask (a stand-in for the user's dialog).
        a.click(Click::Ask);
        a.row();
        a.click(Click::TurnOff);
        assert_eq!(*seen.lock().unwrap(), [false, true, false]);
    }

    #[test]
    fn the_wrapper_never_turns_anything_on_by_itself() {
        let (mut a, _) = rig(false);
        a.row();
        assert!(!a.inner.on);
    }
}
