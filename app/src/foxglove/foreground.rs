//! The program in front, as the `app.foreground` fact, and switching profiles by it
//! (docs/profiles.md, phase 3).
//!
//! Event-driven: the panel crate's foreground hook wakes this thread; nothing polls. Foxglove
//! itself never counts as the program in front, so opening its settings doesn't switch.
//!
//! Rules: the first rule whose program matches picks the profile. Leaving a program that a rule
//! switched for goes back to the base settings. A profile picked by hand holds until the next
//! program that matches a rule.

use std::sync::mpsc;

use super::store::{self, Rule};

/// What the rules decide for a newly focused `program`, given who chose the current profile.
/// `Some(choice)` switches (`choice` = `None` means the base settings).
pub fn decide(rules: &[Rule], program: &str, active: Option<&str>, by_rule: bool) -> Option<Option<String>> {
    match rules.iter().find(|r| r.program.eq_ignore_ascii_case(program)) {
        Some(rule) if active != Some(rule.profile.as_str()) => Some(Some(rule.profile.clone())),
        Some(_) => None,
        None if by_rule && active.is_some() => Some(None),
        None => None,
    }
}

fn own_exe() -> Option<String> {
    std::env::current_exe().ok()?.file_name().map(|n| n.to_string_lossy().to_lowercase())
}

/// Starts watching the foreground program (Windows). `FOXGLOVE_DISABLE=foreground` leaves it out.
pub fn start() {
    if super::repaint_trace::disabled("foreground") {
        return;
    }
    super::services::spawn_named("foreground", || {
        let (tx, rx) = mpsc::channel::<()>();
        let _changes = foxglove_panel::changes::Changes::new(std::sync::Arc::new(move || {
            let _ = tx.send(());
        }));
        let own = own_exe();
        let mut last: Option<String> = None;
        // Whether the active profile came from a rule (so leaving its program reverts).
        let mut by_rule = false;
        for () in rx {
            let Some(program) = foxglove_panel::providers::foreground_exe().map(|e| e.to_lowercase()) else {
                continue;
            };
            if Some(&program) == own.as_ref() || last.as_ref() == Some(&program) {
                continue;
            }
            last = Some(program.clone());
            super::palm_host::bus()
                .facts()
                .set("app.foreground", palm::facts::Value::Text(program.clone()));
            let (auto, rules) = store::rules();
            if !auto {
                continue;
            }
            let (_, active) = store::cached_profiles();
            if let Some(choice) = decide(&rules, &program, active.as_deref(), by_rule) {
                by_rule = choice.is_some();
                if let Err(e) = store::set_active(choice.as_deref()) {
                    eprintln!("Foxglove: can't switch profile: {e}");
                }
            } else if active.is_none() {
                by_rule = false;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(program: &str, profile: &str) -> Rule {
        Rule { program: program.into(), profile: profile.into() }
    }

    #[test]
    fn rules_switch_and_leaving_reverts_only_what_a_rule_chose() {
        let rules = [rule("eldenring.exe", "Game"), rule("code.exe", "Work")];
        assert_eq!(decide(&rules, "EldenRing.exe", None, false), Some(Some("Game".into())));
        assert_eq!(decide(&rules, "eldenring.exe", Some("Game"), true), None, "already there");
        assert_eq!(decide(&rules, "explorer.exe", Some("Game"), true), Some(None), "left the game");
        assert_eq!(decide(&rules, "explorer.exe", Some("Game"), false), None, "a hand pick holds");
        assert_eq!(decide(&rules, "code.exe", Some("Game"), false), Some(Some("Work".into())), "until a rule matches");
        assert_eq!(decide(&rules, "explorer.exe", None, false), None);
    }
}
