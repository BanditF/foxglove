//! The focus timer's Hideout tile and page (docs/hideout-plugin-modules-ux.md): the time left,
//! the phase and round, a meter, and Pause or Resume tied to the `pause` action.

use foxglove_plugin::ui::{self, Button, Press, Tile};
use serde_json::Value;

use crate::timer::{Durations, Phase, Timer};

/// `m:ss`.
pub fn clock(left: u64) -> String {
    format!("{}:{:02}", left / 60, left % 60)
}

/// The tile at `now`; `board` is whether a keyboard is connected.
pub fn tile(t: &Timer, d: &Durations, now: u64, board: bool) -> Tile {
    let left = t.left(now);
    let total = d.of(t.phase).max(1) as f64;
    let paused = t.paused();
    let sub = if paused {
        "Paused".to_string()
    } else if !board {
        "Keyboard not connected".to_string()
    } else {
        match t.phase {
            Phase::Focus => format!("Focus · round {} of {}", t.round, d.rounds),
            Phase::Rest => format!("Rest · then round {}", t.round + 1),
            Phase::LongRest => "Long rest · then round 1".to_string(),
        }
    };
    Tile {
        value: clock(left),
        tone: match (paused, t.phase) {
            (true, _) => "off",
            (false, Phase::Focus) => "active",
            (false, _) => "ok",
        },
        sub: Some(sub),
        meter: (!paused).then_some((total - left as f64, total)),
        button: Some(Button::new(
            if paused { "Resume" } else { "Pause" },
            Press::Action("pause"),
        )),
    }
}

/// The tile and the page, for `ui.register`.
pub fn panels(t: &Timer, d: &Durations, now: u64, board: bool) -> Vec<Value> {
    let tile = tile(t, d, now, board);
    let reset = ui::button(
        "now-reset",
        &Button::new("Start a new focus period", Press::Action("reset")),
    );
    let page = vec![
        ui::now_section(&tile, vec![reset]),
        ui::section(
            "keyboard",
            "On your keyboard",
            vec![
                ui::label(
                    "keyboard-screen",
                    "Info screen: the time left and the next step",
                ),
                ui::label("keyboard-lights", "Lights: red to focus, green to rest"),
            ],
        ),
        ui::section(
            "about",
            "About",
            vec![ui::markdown(
                "about-text",
                &format!(
                    "Alternates {} minutes of focus and {} of rest, with a {}-minute rest every \
                     {} rounds.",
                    d.focus / 60,
                    d.rest / 60,
                    d.long_rest / 60,
                    d.rounds
                ),
            )],
        ),
    ];
    vec![
        ui::panel("tile", "tile", "Focus timer", "timer", tile.tree()),
        ui::panel(
            "page",
            "page",
            "Focus timer",
            "timer",
            ui::column("page", page),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use hideout_contract::module::{Context, Module};

    #[test]
    fn the_panels_match_the_hub_vectors() {
        let m = Module::parse(include_str!("../module.toml"), &Context::default()).unwrap();
        let declared: Vec<String> = m.panels.iter().map(|p| p.id.clone()).collect();
        let actions: Vec<String> = m.actions.iter().map(|a| a.decl.id.clone()).collect();
        let d = Durations {
            focus: 25 * 60,
            rest: 5 * 60,
            long_rest: 15 * 60,
            rounds: 4,
        };
        let mut focus = Timer::start(0, &d);
        focus.round = 2;
        let now = 25 * 60 - (18 * 60 + 42);
        let mut rest = Timer::start(0, &d);
        rest.phase = Phase::Rest;
        rest.deadline = Some(190);
        let mut paused = focus.clone();
        paused.toggle_pause(now);
        foxglove_plugin::ui::check_vectors(
            "focus-timer.json",
            (&m.id, &m.name, &declared, &actions),
            vec![
                ("Focus", panels(&focus, &d, now, true)),
                ("Rest", panels(&rest, &d, 0, true)),
                ("Paused", panels(&paused, &d, now, true)),
                ("Keyboard away", panels(&focus, &d, now, false)),
            ],
        );
    }
}
