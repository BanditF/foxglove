//! "Screens: Dark / Light" under Showing now (#666): the keyboard's mono panels, both halves,
//! every widget and every piece of art. The keyboard keeps the choice (firmware
//! widgets/panel_polarity.h, D7 12, HELLO bit 27) and passes it to the right half itself, so it
//! holds without the app and over a reboot; the app keeps no copy. Shown only when the firmware
//! has it.
//!
//! Polling: none. Start events: the section first shown, a new connection (its generation), a
//! click. Stop: one D7 12 each (retried once if unanswered), then the answer is cached.

use std::sync::Mutex;

use foxglove_link::protocol::{PanelPolarity, Polarity};

use super::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not asked yet on this connection.
    Unknown,
    Asking,
    /// No keyboard, or firmware without the bit: the control isn't drawn.
    Hidden,
    /// The firmware has it but the read went unanswered twice.
    NoAnswer,
    Known(PanelPolarity),
}

static STATE: Mutex<(Option<u64>, State)> = Mutex::new((None, State::Unknown));
/// One D7 12 at a time; a click while one runs is sent after it (the newest wins).
static RUNNING: Mutex<Option<Option<Option<Polarity>>>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// The previews' state, so they never ask a real keyboard.
    pub static PREVIEW: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}

fn set_state(generation: Option<u64>, s: State) {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = (generation, s);
}

/// Reads the polarity (`set` = `None`) or sets it, in the background; `ctx` repaints after.
pub fn ask(set: Option<Polarity>, ctx: egui::Context) {
    let set = set.filter(|_| !super::pause::paused());
    {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(queued) = running.as_mut() {
            *queued = Some(set.or(queued.flatten()));
            return;
        }
        *running = Some(None);
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(s.1, State::Known(_)) {
            s.1 = State::Asking;
        }
    }
    let _ = super::services::spawn_supervised("panel-polarity", move || {
        let mut set = set;
        loop {
            request(set, &ctx);
            let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
            match *running {
                Some(Some(next)) => {
                    set = next;
                    *running = Some(None);
                }
                _ => {
                    *running = None;
                    break;
                }
            }
        }
    });
}

fn request(set: Option<Polarity>, ctx: &egui::Context) {
    let (generation, state) = match super::devices::device() {
        None => (super::devices::generation(), State::Hidden),
        Some(d) if !d.panel_polarity_supported() => {
            (Some(d.link().state().generation), State::Hidden)
        }
        Some(d) => {
            let generation = Some(d.link().state().generation);
            let mut state = State::NoAnswer;
            for attempt in 0..2 {
                match d.panel_polarity(set) {
                    Ok(p) => {
                        if let Some(asked) = set {
                            eprintln!(
                                "Foxglove: panel polarity set to {asked:?}: the keyboard has {:?}, the right half {:?} so far",
                                p.polarity, p.right
                            );
                        }
                        state = State::Known(p);
                        break;
                    }
                    Err(foxglove_devices::DeviceError::Unsupported) => {
                        state = State::Hidden;
                        break;
                    }
                    Err(e) if attempt == 1 => {
                        eprintln!("Foxglove: panel polarity: {}", super::devices::describe(&e));
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_secs(1)),
                }
            }
            if state == State::NoAnswer {
                if let State::Known(_) = current() {
                    state = current();
                }
            }
            (generation, state)
        }
    };
    set_state(generation, state);
    ctx.request_repaint();
}

fn current() -> State {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).1
}

/// The cached state; asks on first look and after each new connection.
fn state(ctx: &egui::Context) -> State {
    let generation = super::devices::generation();
    let (seen, s) = *STATE.lock().unwrap_or_else(|e| e.into_inner());
    let stale = s == State::Unknown
        || (seen != generation && s != State::Asking)
        || (generation.is_none() && !matches!(s, State::Hidden));
    if stale && generation.is_some() {
        ask(None, ctx.clone());
    } else if generation.is_none() && s != State::Hidden {
        set_state(None, State::Hidden);
        return State::Hidden;
    }
    s
}

/// The control, inside Showing now.
pub fn show(ui: &mut egui::Ui) {
    #[cfg(test)]
    if let Some(s) = PREVIEW.with_borrow(|v| *v) {
        draw(ui, &s);
        return;
    }
    let ctx = ui.ctx().clone();
    let s = state(&ctx);
    if let Some(p) = draw(ui, &s) {
        ask(Some(p), ctx);
    }
}

/// Paints the two choices; returns the one clicked this frame.
pub fn draw(ui: &mut egui::Ui, s: &State) -> Option<Polarity> {
    let mut out = None;
    let known = match s {
        State::Hidden | State::Unknown => return None,
        State::Asking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Screens: asking the keyboard…");
            });
            return None;
        }
        State::NoAnswer => {
            ui.horizontal_wrapped(|ui| {
                ui.weak("Screens: the keyboard didn't answer");
                if ui.small_button("↻").on_hover_text("Refresh").clicked() {
                    ask(None, ui.ctx().clone());
                }
            });
            return None;
        }
        State::Known(p) => *p,
    };
    ui.horizontal_wrapped(|ui| {
        ui.label("Screens");
        for (p, text, hint) in [
            (Polarity::Dark, "Dark", "Light drawing on a dark screen."),
            (Polarity::Light, "Light", "Dark drawing on a light screen."),
        ] {
            let on = known.polarity == Some(p);
            if theme::pill(ui, on, text)
                .on_hover_text(format!(
                    "{hint} Both halves, every widget and every piece of art. Saved on the \
                     keyboard, so it holds without Foxglove."
                ))
                .clicked()
                && !on
            {
                out = Some(p);
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(polarity: Polarity, right: Option<Polarity>) -> State {
        State::Known(PanelPolarity {
            status: 0,
            polarity: Some(polarity),
            default: Some(Polarity::Dark),
            right,
        })
    }

    /// Hidden without the firmware bit; the two choices with it, the shown one selected.
    #[test]
    fn the_control_shows_only_with_the_capability() {
        let ctx = egui::Context::default();
        let mut drew = Vec::new();
        for s in [State::Hidden, known(Polarity::Light, None)] {
            let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| {
                let before = ui.cursor().top();
                assert_eq!(draw(ui, &s), None, "nothing clicked");
                drew.push(ui.cursor().top() > before);
            });
            frame.textures_delta.clear();
        }
        assert_eq!(drew, [false, true]);
    }
}
