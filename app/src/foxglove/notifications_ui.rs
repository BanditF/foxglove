//! The Screens page's **Notifications** card (#507): the opt-in, the light cue, and the apps
//! whose text may show, each ticked on its own. One hint when Windows doesn't give access.
//! Changes are saved to the store, which the notification service watches.

use std::cell::RefCell;

use super::notifications::{self, NotifySettings, State};

thread_local! {
    /// The settings as last loaded or saved here; reloaded when the page opens again.
    static SETTINGS: RefCell<Option<NotifySettings>> = const { RefCell::new(None) };
}

/// The card, drawn with the left screen's (only while a keyboard with the host link answers).
pub fn card(ui: &mut egui::Ui) {
    if super::screens_ui::snapshot().hello.is_none() {
        SETTINGS.with_borrow_mut(|s| *s = None);
        return;
    }
    super::settings_ui::section(ui, "Notifications", |ui| {
        SETTINGS.with_borrow_mut(|cached| {
            let settings = cached.get_or_insert_with(NotifySettings::load);
            if contents(
                ui,
                settings,
                &notifications::state(),
                &notifications::recent_apps(),
            ) {
                if let Err(e) = settings.save() {
                    ui.colored_label(ui.visuals().error_fg_color, format!("Can't save: {e}"));
                }
            }
        });
    });
}

/// What the state says, when it's something other than on, off or the access hint.
fn state_line(state: &State) -> Option<String> {
    match state {
        State::Unavailable(e) => Some(format!("Not available: {e}")),
        _ => None,
    }
}

/// The card's contents; true when a setting changed. Also drawn by the previews.
pub(crate) fn contents(
    ui: &mut egui::Ui,
    settings: &mut NotifySettings,
    state: &State,
    recent: &[String],
) -> bool {
    let mut changed = false;
    changed |= ui
        .checkbox(
            &mut settings.enabled,
            "Show Windows notifications on the left screen",
        )
        .changed();
    ui.weak(
        "The app's name, for a few seconds. Nothing is kept. Not while the PC is locked, \
         in Do not disturb, or in a full-screen game.",
    );
    if settings.enabled && *state == State::NoAccess {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                super::theme::palette(ui).warn,
                "Windows doesn't let Foxglove see notifications.",
            );
            if ui.button("Open Windows settings").clicked() {
                open_privacy_settings();
            }
        });
    } else if let Some(line) = state_line(state).filter(|_| settings.enabled) {
        ui.weak(line);
    }
    ui.add_enabled_ui(settings.enabled, |ui| {
        changed |= ui
            .checkbox(
                &mut settings.light_cue,
                "Light cue: a short glow on the keyboard",
            )
            .on_hover_text("Over a scene; a muted microphone's light stays")
            .changed();
        ui.add_space(6.0);
        ui.label(super::theme::caps_label(ui, "Show the text for"));
        ui.weak("Off for every app until you tick it.");
        let mut apps: Vec<String> = recent.to_vec();
        for app in &settings.text_apps {
            if !apps.contains(app) {
                apps.push(app.clone());
            }
        }
        if apps.is_empty() {
            ui.weak("Apps show here after they send a notification.");
        }
        for app in apps {
            let mut on = settings.text_apps.contains(&app);
            if ui.checkbox(&mut on, app.as_str()).changed() {
                if on {
                    settings.text_apps.insert(app);
                } else {
                    settings.text_apps.remove(&app);
                }
                changed = true;
            }
        }
    });
    changed
}

/// Windows' notification privacy page, where access is turned on.
fn open_privacy_settings() {
    #[cfg(windows)]
    // SAFETY: every string is a static wide literal and the window handle is none; ShellExecuteW
    // keeps nothing after it returns.
    unsafe {
        use windows::core::w;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        ShellExecuteW(
            None,
            w!("open"),
            w!("ms-settings:privacy-notifications"),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}
