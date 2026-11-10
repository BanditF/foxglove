// SPDX-License-Identifier: MIT
//! The tray icon and its fixed menu frame (design UX8): a status line, Open Hideout; then the
//! pinned quick actions (none until modules declare actions); then Pause Hideout, Open log
//! folder, Quit Hideout. The icon has three states (`icon::State`).
//!
//! tray-icon delivers its events on the thread that made the icon (the UI thread), from inside
//! the Win32 message loop, so they arrive even while the window is hidden and draws no frames.

use crate::icon::{self, State};
use std::cell::RefCell;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    Pause(bool),
    OpenLogs,
    Quit,
}

const OPEN: &str = "hideout.open";
const PAUSE: &str = "hideout.pause";
const LOGS: &str = "hideout.logs";
const QUIT: &str = "hideout.quit";

struct Tray {
    icon: TrayIcon,
    status: MenuItem,
    pause: CheckMenuItem,
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

fn glyph(state: State) -> Option<Icon> {
    Icon::from_rgba(icon::rgba(32, state), 32, 32).ok()
}

/// The one log line for an install attempt.
pub fn outcome_line(result: &Result<(), String>) -> String {
    match result {
        Ok(()) => "tray icon registered".to_string(),
        Err(e) => format!("tray icon failed: {e}"),
    }
}

#[cfg(test)]
/// True while the icon is alive (kept in this thread's slot, so it is not dropped).
pub fn is_installed() -> bool {
    TRAY.with(|t| t.borrow().is_some())
}

/// Makes the tray icon on the UI thread and logs the outcome in one line.
pub fn install(on_command: impl Fn(Command) + Send + Sync + 'static) -> Result<(), String> {
    let result = install_inner(on_command);
    match &result {
        Ok(()) => log::info!("{}", outcome_line(&result)),
        Err(_) => log::error!("{}", outcome_line(&result)),
    }
    result
}

/// `on_command` runs for each menu pick and click.
fn install_inner(on_command: impl Fn(Command) + Send + Sync + 'static) -> Result<(), String> {
    let status = MenuItem::new("No modules yet", false, None);
    let open = MenuItem::with_id(OPEN, "Open Hideout", true, None);
    let pause = CheckMenuItem::with_id(PAUSE, "Pause Hideout", true, false, None);
    let logs = MenuItem::with_id(LOGS, "Open log folder", true, None);
    let quit = MenuItem::with_id(QUIT, "Quit Hideout", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &status,
        &open,
        &PredefinedMenuItem::separator(),
        &pause,
        &logs,
        &quit,
    ])
    .map_err(|e| e.to_string())?;
    let icon = TrayIconBuilder::new()
        .with_tooltip(crate::PRODUCT)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_icon(glyph(State::Normal).ok_or("tray glyph")?)
        .build()
        .map_err(|e| e.to_string())?;
    TRAY.with(|t| {
        *t.borrow_mut() = Some(Tray {
            icon,
            status,
            pause,
        })
    });

    let on_command = std::sync::Arc::new(on_command);
    let menu_cmd = on_command.clone();
    MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
        let cmd = match ev.id.0.as_str() {
            OPEN => Command::Open,
            PAUSE => Command::Pause(
                TRAY.with(|t| t.borrow().as_ref().is_some_and(|t| t.pause.is_checked())),
            ),
            LOGS => Command::OpenLogs,
            QUIT => Command::Quit,
            _ => return,
        };
        menu_cmd(cmd);
    }));
    TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| match ev {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
        | TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        } => on_command(Command::Open),
        _ => {}
    }));
    Ok(())
}

/// Sets the glyph and the menu's status line. A no-op off the UI thread or before `install`.
pub fn set_state(state: State, status_line: &str) {
    TRAY.with(|t| {
        if let Some(t) = t.borrow().as_ref() {
            let _ = t.icon.set_icon(glyph(state));
            t.status.set_text(status_line);
            t.pause.set_checked(state == State::Paused);
        }
    });
}

/// Removes the icon now (before a quick exit, so no ghost icon stays in the tray).
pub fn remove() {
    TRAY.with(|t| drop(t.borrow_mut().take()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_lines() {
        assert_eq!(outcome_line(&Ok(())), "tray icon registered");
        assert_eq!(outcome_line(&Err("boom".into())), "tray icon failed: boom");
    }

    #[test]
    fn glyphs_build_for_every_state() {
        for s in [State::Normal, State::Paused, State::NeedsYou] {
            assert!(glyph(s).is_some());
        }
        assert!(!is_installed());
    }
}
