// SPDX-License-Identifier: MIT
//! The tray icon and its fixed menu frame (design UX8): a status line, Open Hideout; then the
//! pinned quick actions (none until modules declare actions); then Pause Hideout, Open log
//! folder, Quit Hideout. The icon has three states (`icon::State`). In safe mode **Start
//! normally** comes right under the status line (docs/hideout-supervisor-ux.md, column Tr).
//! While a search for the module that breaks Hideout runs (#1033), **Start round k** (between
//! rounds) and **Stop finding** come right after Open Hideout, and Start normally goes.
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
    /// A left click on the icon: open or close the tray flyout above the icon (its rectangle,
    /// physical pixels).
    Flyout(crate::placement::Rect),
    /// A double click on the icon: the flyout goes at once (not drawn again), then Hideout's
    /// window opens (#997, #1076).
    DoubleClick,
    Pause(bool),
    OpenLogs,
    Quit,
    /// **Start normally** (safe mode only): safe mode ends and the enabled modules start.
    StartNormally,
    /// **Start round k** (between rounds of a search, #1033).
    StartRound,
    /// **Stop finding** (during a search).
    StopFinding,
}

const OPEN: &str = "hideout.open";
const PAUSE: &str = "hideout.pause";
const LOGS: &str = "hideout.logs";
const QUIT: &str = "hideout.quit";
const START_NORMALLY: &str = "hideout.start-normally";
const START_ROUND: &str = "hideout.start-round";
const STOP_FINDING: &str = "hideout.stop-finding";
/// The menu item's words (the banner's button says the same).
pub const START_NORMALLY_LABEL: &str = hideout_supervisor::words::START_NORMALLY;

struct Tray {
    icon: TrayIcon,
    menu: Menu,
    status: MenuItem,
    pause: CheckMenuItem,
    start_normally: MenuItem,
    /// Whether `start_normally` is in the menu now.
    safe_shown: bool,
    /// The search's items (#1033) and whether each is in the menu now.
    start_round: MenuItem,
    stop_finding: MenuItem,
    round_shown: bool,
    stop_shown: bool,
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

/// The icon's message window (tray-icon's), for lab control (#1003); 0 while there is no icon.
static MESSAGE_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// The icon's message window, any thread; 0 while there is no icon.
pub fn message_window() -> isize {
    MESSAGE_WINDOW.load(std::sync::atomic::Ordering::Acquire)
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
    let status = MenuItem::new(crate::marks::QUIET_LINE, false, None);
    let open = MenuItem::with_id(OPEN, "Open Hideout", true, None);
    let pause = CheckMenuItem::with_id(PAUSE, "Pause Hideout", true, false, None);
    let logs = MenuItem::with_id(LOGS, "Open log folder", true, None);
    let quit = MenuItem::with_id(QUIT, "Quit Hideout", true, None);
    let start_normally = MenuItem::with_id(START_NORMALLY, START_NORMALLY_LABEL, true, None);
    let start_round = MenuItem::with_id(START_ROUND, "", true, None);
    let stop_finding = MenuItem::with_id(
        STOP_FINDING,
        hideout_supervisor::bisect::words::STOP,
        true,
        None,
    );
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
        .with_menu(Box::new(menu.clone()))
        .with_menu_on_left_click(false)
        .with_icon(glyph(State::Normal).ok_or("tray glyph")?)
        .build()
        .map_err(|e| e.to_string())?;
    MESSAGE_WINDOW.store(
        icon.window_handle() as isize,
        std::sync::atomic::Ordering::Release,
    );
    TRAY.with(|t| {
        *t.borrow_mut() = Some(Tray {
            icon,
            menu,
            status,
            pause,
            start_normally,
            safe_shown: false,
            start_round,
            stop_finding,
            round_shown: false,
            stop_shown: false,
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
            START_NORMALLY => Command::StartNormally,
            START_ROUND => Command::StartRound,
            STOP_FINDING => Command::StopFinding,
            _ => return,
        };
        menu_cmd(cmd);
    }));
    TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| match ev {
        // A click opens the tray flyout (#991); a double click opens Hideout itself, and the
        // flyout never shows on the way (#997, #1076): see `ClickGate`.
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Down,
            ..
        } => GATE.with(|g| g.borrow_mut().down()),
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            rect,
            ..
        } => {
            if let Some(cmd) = GATE.with(|g| g.borrow_mut().up(icon_rect(&rect))) {
                on_command(cmd)
            }
        }
        TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        } => on_command(GATE.with(|g| g.borrow_mut().double())),
        // tray-icon arms a 15 ms leave-check timer on each move over the icon and only kills it
        // when a tick still holds a fresh position; after the menu opens (the cursor leaves
        // through the menu, so no move clears it) it ticks at 64 Hz for good. The hub uses no
        // Enter/Leave events, so the timer goes as soon as it is armed (#935).
        TrayIconEvent::Move { .. } => stop_leave_timer(),
        _ => {}
    }));
    Ok(())
}

/// The icon's rectangle in physical pixels.
fn icon_rect(r: &tray_icon::Rect) -> crate::placement::Rect {
    crate::placement::Rect {
        x: r.position.x.round() as i32,
        y: r.position.y.round() as i32,
        w: r.size.width as i32,
        h: r.size.height as i32,
    }
}

thread_local! {
    /// The left button on the icon (UI thread only, where tray-icon delivers its events).
    static GATE: RefCell<ClickGate> = RefCell::new(ClickGate::default());
}

/// The left button on the tray icon (#997, #1076). Windows sends a double click as down, up,
/// `WM_LBUTTONDBLCLK`, up, and tray-icon makes a click of each up. The first up opens the
/// flyout at once, as any click does; the double click then sends [`Command::DoubleClick`],
/// which takes the flyout away in the same handler before Hideout shows; the up that ends the
/// double click is not a click of its own, so it never opens or toggles the flyout again.
#[derive(Debug, Default)]
pub(crate) struct ClickGate {
    /// A double click came: the next up ends it.
    swallow_up: bool,
}

impl ClickGate {
    pub(crate) fn down(&mut self) {
        self.swallow_up = false;
    }

    /// An up: the flyout toggles at once, unless it ends a double click.
    pub(crate) fn up(&mut self, icon: crate::placement::Rect) -> Option<Command> {
        if std::mem::take(&mut self.swallow_up) {
            return None;
        }
        Some(Command::Flyout(icon))
    }

    /// `WM_LBUTTONDBLCLK`.
    pub(crate) fn double(&mut self) -> Command {
        self.swallow_up = true;
        Command::DoubleClick
    }
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

/// Sets the glyph, the menu's status line and the icon's tooltip. A no-op off the UI thread or
/// before `install`.
pub fn set_state(state: State, status_line: &str, tooltip: &str) {
    TRAY.with(|t| {
        if let Some(t) = t.borrow().as_ref() {
            let _ = t.icon.set_icon(glyph(state));
            let _ = t.icon.set_tooltip(Some(tooltip));
            t.status.set_text(status_line);
            t.pause.set_checked(state == State::Paused);
        }
    });
}

/// Puts **Start normally** under the status line in safe mode and takes it out after. A no-op
/// off the UI thread or before `install`.
pub fn set_safe_mode(on: bool) {
    TRAY.with(|t| {
        if let Some(t) = t.borrow_mut().as_mut() {
            if on == t.safe_shown {
                return;
            }
            let done = if on {
                t.menu.insert(&t.start_normally, 1)
            } else {
                t.menu.remove(&t.start_normally)
            };
            match done {
                Ok(()) => t.safe_shown = on,
                Err(e) => log::warn!("tray: Start normally item: {e}"),
            }
        }
    });
}

/// The search's items (#1033): **Start round k** between rounds and **Stop finding** while it
/// runs, right after Open Hideout; `None` takes them out. A no-op off the UI thread or before
/// `install`.
pub fn set_search(s: Option<&hideout_supervisor::bisect::Tray>) {
    TRAY.with(|t| {
        let mut g = t.borrow_mut();
        let Some(t) = g.as_mut() else { return };
        // Out first, then in again at their places with the words of now.
        if t.round_shown && t.menu.remove(&t.start_round).is_ok() {
            t.round_shown = false;
        }
        if t.stop_shown && t.menu.remove(&t.stop_finding).is_ok() {
            t.stop_shown = false;
        }
        // Status line, Start normally when shown, Open Hideout.
        let mut at = 2 + usize::from(t.safe_shown);
        if let Some(label) = s.and_then(|s| s.start_next.as_deref()) {
            t.start_round.set_text(label);
            match t.menu.insert(&t.start_round, at) {
                Ok(()) => {
                    t.round_shown = true;
                    at += 1;
                }
                Err(e) => log::warn!("tray: Start round item: {e}"),
            }
        }
        if s.is_some_and(|s| s.stop) {
            match t.menu.insert(&t.stop_finding, at) {
                Ok(()) => t.stop_shown = true,
                Err(e) => log::warn!("tray: Stop finding item: {e}"),
            }
        }
    });
}

/// Removes the icon now (before a quick exit, so no ghost icon stays in the tray).
pub fn remove() {
    MESSAGE_WINDOW.store(0, std::sync::atomic::Ordering::Release);
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
        // Lab control (#1003) posts tray-icon's callback message, private as well.
        assert_eq!(crate::lab_control::WM_USER_TRAYICON, 6002);
    }

    const ICON: crate::placement::Rect = crate::placement::Rect {
        x: 1800,
        y: 1044,
        w: 24,
        h: 24,
    };

    /// #997 / #1076: a double click as Windows (and lab control's `tray-double-click`) sends
    /// it: down, up, `WM_LBUTTONDBLCLK`, up. The first up opens the flyout at once (no wait,
    /// as any single click); the double click is a `DoubleClick` (the shell takes the flyout
    /// away then); the up that ends it is no click, so the flyout isn't asked for again.
    #[test]
    fn a_double_click_asks_once_then_takes_the_flyout_away() {
        let mut g = ClickGate::default();
        g.down();
        assert_eq!(g.up(ICON), Some(Command::Flyout(ICON)));
        assert_eq!(g.double(), Command::DoubleClick);
        assert_eq!(
            g.up(ICON),
            None,
            "the up ending the double click toggled the flyout"
        );
        // The next click is a click again, at once.
        g.down();
        assert_eq!(g.up(ICON), Some(Command::Flyout(ICON)));
        assert_eq!(g.up(ICON), Some(Command::Flyout(ICON)));
    }

    #[test]
    fn outcome_lines() {
        assert_eq!(outcome_line(&Ok(())), "tray icon registered");
        assert_eq!(outcome_line(&Err("boom".into())), "tray icon failed: boom");
    }

    #[test]
    fn glyphs_build_for_every_state() {
        for s in [
            State::Normal,
            State::Paused,
            State::NeedsYou,
            State::SafeMode,
        ] {
            assert!(glyph(s).is_some());
        }
        assert!(!is_installed());
    }
}
