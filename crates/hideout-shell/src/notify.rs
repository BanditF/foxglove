// SPDX-License-Identifier: MIT
//! The hub's system notifications (docs/hideout-supervisor-ux.md, "Notifications"): Windows
//! toasts under the hub's AUMID, with up to three buttons.
//!
//! No COM activator: the buttons are answered through the toast's `Activated` event, which comes
//! while the hub runs (it always does: it lives in the tray). A click after the hub has quit opens
//! nothing. An unpackaged app's toasts show only when Windows knows its AUMID, so the first toast
//! writes the per-user registration (`HKCU\Software\Classes\AppUserModelId\<AUMID>`, a display
//! name only), as an installer would.
//!
//! Tests build the XML only; nothing in a test shows a notification.

use std::sync::{Arc, Mutex};

/// What a button click hands back: the button's argument.
pub type OnClick = Arc<dyn Fn(&str) + Send + Sync>;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The toast's XML: title, body, and a button per `(label, argument)`. A click on the toast
/// itself hands back `body_arg`.
pub fn toast_xml(title: &str, body: &str, buttons: &[(String, String)], body_arg: &str) -> String {
    let mut x = format!(
        "<toast launch=\"{}\"><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual>",
        escape(body_arg),
        escape(title),
        escape(body)
    );
    if !buttons.is_empty() {
        x.push_str("<actions>");
        for (label, arg) in buttons.iter().take(3) {
            x.push_str(&format!(
                "<action content=\"{}\" arguments=\"{}\"/>",
                escape(label),
                escape(arg)
            ));
        }
        x.push_str("</actions>");
    }
    x.push_str("</toast>");
    x
}

/// Toasts shown this run, kept so their `Activated` handlers stay alive.
static SHOWN: Mutex<Vec<windows::UI::Notifications::ToastNotification>> = Mutex::new(Vec::new());

/// Shows a toast on its own short-lived thread (WinRT needs an initialised apartment; the
/// supervisor's threads have none). Errors go to the log.
pub fn toast(
    title: String,
    body: String,
    buttons: Vec<(String, String)>,
    body_arg: String,
    on_click: OnClick,
) {
    let spawned = std::thread::Builder::new()
        .name("hideout-toast".into())
        .spawn(move || {
            if let Err(e) = show(&title, &body, &buttons, &body_arg, on_click) {
                log::warn!("notification {title:?} not shown: {e}");
            }
        });
    if let Err(e) = spawned {
        log::warn!("notification thread: {e}");
    }
}

fn show(
    title: &str,
    body: &str,
    buttons: &[(String, String)],
    body_arg: &str,
    on_click: OnClick,
) -> windows::core::Result<()> {
    use windows::core::{Interface, HSTRING};
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
    use windows::UI::Notifications::{
        ToastActivatedEventArgs, ToastNotification, ToastNotificationManager,
    };
    // Already initialised (or initialised otherwise) is fine.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    register_aumid();
    let xml = XmlDocument::new()?;
    xml.LoadXml(&HSTRING::from(toast_xml(title, body, buttons, body_arg)))?;
    let toast = ToastNotification::CreateToastNotification(&xml)?;
    toast.Activated(&TypedEventHandler::new(
        move |_, args: windows::core::Ref<windows::core::IInspectable>| {
            if let Some(a) = args.as_ref() {
                if let Ok(t) = a.cast::<ToastActivatedEventArgs>() {
                    on_click(&t.Arguments()?.to_string());
                }
            }
            Ok(())
        },
    ))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(crate::AUMID))?
        .Show(&toast)?;
    let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
    shown.push(toast);
    // A few are enough: an older toast's buttons stop answering once it leaves the list.
    if shown.len() > 8 {
        shown.remove(0);
    }
    Ok(())
}

/// Windows shows an unpackaged app's toasts only for a known AUMID: its display name under
/// `HKCU\Software\Classes\AppUserModelId\<AUMID>`. Written once per run, before the first toast.
fn register_aumid() {
    use std::sync::Once;
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let key = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{}", crate::AUMID));
        let name: Vec<u16> = crate::PRODUCT.encode_utf16().chain([0]).collect();
        let r = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &key,
                &HSTRING::from("DisplayName"),
                REG_SZ.0,
                Some(name.as_ptr().cast()),
                (name.len() * 2) as u32,
            )
        };
        if r.is_err() {
            log::warn!("notification name not registered: {r:?}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stopped_toast_has_its_two_buttons() {
        let (t, b, a) = hideout_supervisor::words::stopped_notification("Art & Co");
        let buttons: Vec<(String, String)> = a
            .into_iter()
            .zip(["start-again", "open"])
            .map(|(l, k)| (l, format!("{k}:m.art")))
            .collect();
        let x = toast_xml(&t, &b, &buttons, "open:m.art");
        assert!(x.contains("<text>Art &amp; Co stopped</text>"), "{x}");
        assert!(x.contains("<text>It crashed 5 times in 10 minutes.</text>"));
        assert!(x.contains("<action content=\"Start again\" arguments=\"start-again:m.art\"/>"));
        assert!(x.contains("<action content=\"Open Art &amp; Co\" arguments=\"open:m.art\"/>"));
        assert!(x.starts_with("<toast launch=\"open:m.art\">"));
    }
}
