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
        // tray-icon arms a 15 ms leave-check timer on each move over the icon and only kills it
        // when a tick still holds a fresh position; after the menu opens (the cursor leaves
        // through the menu, so no move clears it) it ticks at 64 Hz for good. The hub uses no
        // Enter/Leave events, so the timer goes as soon as it is armed (#935).
        TrayIconEvent::Move { .. } => stop_leave_timer(),
        _ => {}
    }));
    Ok(())
}

/// tray-icon 0.24's private timer id for its leave check (`WM_USER_LEAVE_TIMER_ID`).
const LEAVE_TIMER_ID: usize = 6007;

/// Kills tray-icon's leave-check timer on the icon's message window. Called from inside the
/// icon's window procedure, right after tray-icon armed it, so the timer never ticks.
fn stop_leave_timer() {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::KillTimer;
    TRAY.with(|t| {
        if let Ok(t) = t.try_borrow() {
            if let Some(t) = t.as_ref() {
                let hwnd = HWND(t.icon.window_handle() as *mut _);
                let _ = unsafe { KillTimer(Some(hwnd), LEAVE_TIMER_ID) };
            }
        }
    });
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

    /// `LEAVE_TIMER_ID` is tray-icon 0.24's private constant: a bump must re-check it.
    #[test]
    fn leave_timer_id_matches_the_locked_tray_icon() {
        let lock = include_str!("../../../Cargo.lock");
        let at = lock
            .find("name = \"tray-icon\"")
            .expect("tray-icon in Cargo.lock");
        let version = lock[at..].lines().nth(1).unwrap_or_default();
        assert!(
            version.starts_with("version = \"0.24."),
            "tray-icon changed ({version}): re-check LEAVE_TIMER_ID"
        );
        assert_eq!(LEAVE_TIMER_ID, 6007);
    }

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
