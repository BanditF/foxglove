//! Foxglove's own notifications to the user: the one place they go out. Today: low battery on a
//! half or another device (`battery`, `peripherals`), a firmware update done or failed
//! (`firmware_update`), and a new health warning or failure (`health_notice`).
//!
//! - **On the wallpaper first:** while a paired live wallpaper is connected, not paused, and
//!   said `caps: ["notify"]` in its `hi`, the notification goes to it over the live feed
//!   (`live_feed`, `{"t":"notify"}`). It answers `notified` once it drew it.
//! - **Never lost:** no answer within [`live_wire::ACK_LIMIT`](super::live_wire::ACK_LIMIT), no
//!   such wallpaper, the feed quiet (Foxglove paused, the PC locked, a full-screen game) or the
//!   setting off: the Windows notification shows, as before. Windows' own focus or game mode
//!   may hold that back, as it does for any app.
//! - The left screen's notifications (`notifications`) mirror other apps' Windows notifications
//!   and are untouched.
//!
//! Tests never show a real Windows notification: under `cfg(test)` [`toast`] records instead.

use super::live_wire::Level;

/// One notification, as the wallpaper or Windows shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub title: String,
    pub body: String,
    pub level: Level,
}

/// What a notification is about, for its log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Battery,
    Firmware,
    Health,
}

/// Shows a notification: on the wallpaper when one is there to show it, else as a Windows
/// notification.
pub fn show(kind: Kind, title: &str, body: &str, level: Level) {
    let note = Note {
        title: title.to_string(),
        body: body.to_string(),
        level,
    };
    // The sitting checks look for `battery notice: ` (tests/sitting-parity.toml), whichever way
    // it then goes.
    match kind {
        Kind::Battery => eprintln!("Foxglove: battery notice: {title}"),
        Kind::Firmware => eprintln!("Foxglove: firmware notice: {title}"),
        Kind::Health => eprintln!("Foxglove: health notice: {title}"),
    }
    if let Err(note) = route(note) {
        toast(&note);
    }
}

#[cfg(not(test))]
fn route(note: Note) -> Result<(), Note> {
    super::live_feed::notify(note)
}

/// Tests go straight to the fake sink unless the thread asked for the live feed (its end-to-end
/// test), so a test running beside it never sees its notes routed.
#[cfg(test)]
fn route(note: Note) -> Result<(), Note> {
    if test_toasts::ROUTE.get() {
        super::live_feed::notify(note)
    } else {
        Err(note)
    }
}

/// The Windows notification (the fallback). Under `cfg(test)` it's only recorded.
pub fn toast(note: &Note) {
    #[cfg(test)]
    {
        test_toasts::record(note);
    }
    #[cfg(not(test))]
    {
        imp::show(&note.title, &note.body);
    }
}

/// The fake toast sink: what tests would have shown.
#[cfg(test)]
pub mod test_toasts {
    use super::Note;
    use std::cell::{Cell, RefCell};
    use std::sync::Mutex;

    static SHOWN: Mutex<Vec<Note>> = Mutex::new(Vec::new());
    thread_local! {
        /// This thread's toasts (tests run in parallel, each on its own thread).
        static MINE: RefCell<Vec<Note>> = const { RefCell::new(Vec::new()) };
        /// This thread's notes go through the live feed (the end-to-end test).
        pub static ROUTE: Cell<bool> = const { Cell::new(false) };
    }

    pub fn record(note: &Note) {
        SHOWN
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(note.clone());
        MINE.with(|m| m.borrow_mut().push(note.clone()));
    }

    /// The toasts this thread showed, taken.
    pub fn take_mine() -> Vec<Note> {
        MINE.with(|m| std::mem::take(&mut *m.borrow_mut()))
    }

    /// The recorded toasts whose title starts with `prefix` (tests run in parallel, so each
    /// looks only at its own).
    pub fn titled(prefix: &str) -> Vec<Note> {
        SHOWN
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|n| n.title.starts_with(prefix))
            .cloned()
            .collect()
    }
}

/// The XML-escaped text for a toast.
#[cfg_attr(any(not(windows), test), allow(dead_code))]
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg_attr(any(not(windows), test), allow(dead_code))]
fn toast_xml(title: &str, text: &str) -> String {
    format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        escape(title),
        escape(text)
    )
}

#[cfg(all(windows, not(test)))]
mod imp {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    /// The app id the toasts carry; registered under HKCU so an unpackaged app may show them.
    const APP_ID: &str = "BanditF.Foxglove";

    /// `HKCU\Software\Classes\AppUserModelId\BanditF.Foxglove`: the name and icon Windows shows.
    fn register() -> std::io::Result<()> {
        let (key, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}"))?;
        key.set_value("DisplayName", &"Foxglove")?;
        if let Some(dir) =
            super::super::store::path().and_then(|p| p.parent().map(|d| d.to_path_buf()))
        {
            let icon = dir.join("notification-icon.png");
            if !icon.exists() {
                std::fs::create_dir_all(&dir)?;
                std::fs::write(&icon, include_bytes!("../../resources/icon.png"))?;
            }
            key.set_value("IconUri", &icon.to_string_lossy().to_string())?;
        }
        Ok(())
    }

    pub fn show(title: &str, text: &str) {
        if let Err(e) = register() {
            eprintln!("Foxglove: can't register for notifications: {e}");
        }
        let shown = (|| -> windows::core::Result<()> {
            let xml = XmlDocument::new()?;
            xml.LoadXml(&HSTRING::from(super::toast_xml(title, text)))?;
            let toast = ToastNotification::CreateToastNotification(&xml)?;
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?
                .Show(&toast)
        })();
        match shown {
            Ok(()) => eprintln!("Foxglove: notice as a Windows notification: {title}"),
            Err(e) => eprintln!("Foxglove: can't show the notice: {e}"),
        }
    }
}

#[cfg(all(not(windows), not(test)))]
mod imp {
    pub fn show(title: &str, _text: &str) {
        eprintln!("Foxglove: notice as a Windows notification: {title}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_text_is_escaped() {
        let xml = toast_xml("A & B", "<5%>");
        assert!(xml.contains("<text>A &amp; B</text><text>&lt;5%&gt;</text>"));
    }
}
