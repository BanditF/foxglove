//! Windows' notifications, read for the left screen (#507): the listener's access, the toasts in
//! the notification centre, the wake when one arrives, and whether Windows is holding banners
//! back (Do not disturb or Focus assist, a full-screen game, a presentation).
//!
//! **Package identity.** The arrival event, `UserNotificationListener.NotificationChanged`,
//! needs it (#179): without identity Windows refuses the handler with ERROR_NOT_FOUND
//! (0x80070490), while the access check and the read work (measured 2026-10-27, Windows 11
//! 10.0.26300, from an unpackaged process). So the event is tried first and is what a packaged
//! Foxglove gets; without identity the wake is a write in the notification centre's own database
//! folder (`%LOCALAPPDATA%\Microsoft\Windows\Notifications`), which Windows makes when a toast
//! arrives or goes. Each wake leads to one read (`notifications` settles and bounds them).
//!
//! **Quiet.** Do not disturb / Focus assist comes from the shell's own quiet-hours state
//! (WNF, read through ntdll; undocumented but stable since Windows 10 1803, and only read when a
//! notification arrives), and full screen, presentation mode and quiet time from
//! `SHQueryUserNotificationState`. Either read failing counts as not quiet.

use std::sync::Arc;

use super::notifications::{Access, Quiet, Toast};

/// The wake on arrivals, while it's kept.
pub struct Arrivals {
    /// True when Windows' own event wakes it (package identity); false for the folder watch.
    pub by_event: bool,
    _inner: imp::Inner,
}

pub use imp::{access, quiet, request_access, toasts};

/// Wakes `wake` when notifications may have changed: the listener's event when Windows allows
/// it, else the database folder. `Err` when neither is there.
pub fn watch(wake: Arc<dyn Fn() + Send + Sync>) -> Result<Arrivals, String> {
    imp::watch(wake)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::core::{s, w, HRESULT};
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::Management::{
        UserNotificationListener, UserNotificationListenerAccessStatus as Status,
    };
    use windows::UI::Notifications::{
        KnownNotificationBindings, NotificationKinds, UserNotification,
    };

    /// ERROR_NOT_FOUND as an HRESULT: the event without package identity.
    const NO_IDENTITY: HRESULT = HRESULT(0x8007_0490_u32 as i32);

    pub enum Inner {
        Event {
            listener: UserNotificationListener,
            token: i64,
        },
        Folder(#[allow(dead_code)] super::super::screens_watch::Watch),
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            if let Inner::Event { listener, token } = self {
                let _ = listener.RemoveNotificationChanged(*token);
            }
        }
    }

    fn status(s: windows::core::Result<Status>) -> Access {
        match s {
            Ok(Status::Allowed) => Access::Allowed,
            Ok(Status::Denied) => Access::Denied,
            Ok(_) => Access::Unasked,
            Err(e) => Access::Unavailable(e.message()),
        }
    }

    pub fn access() -> Access {
        status(UserNotificationListener::Current().and_then(|l| l.GetAccessStatus()))
    }

    /// Asks Windows for access (it may ask the user once) and waits for the answer.
    pub fn request_access() -> Access {
        status(UserNotificationListener::Current().and_then(|l| l.RequestAccessAsync()?.join()))
    }

    pub fn watch(wake: Arc<dyn Fn() + Send + Sync>) -> Result<Arrivals, String> {
        let listener = UserNotificationListener::Current().map_err(|e| e.message())?;
        let event_wake = wake.clone();
        let handler = TypedEventHandler::new(move |_, _| {
            event_wake();
            Ok(())
        });
        match listener.NotificationChanged(&handler) {
            Ok(token) => {
                return Ok(Arrivals {
                    by_event: true,
                    _inner: Inner::Event { listener, token },
                })
            }
            Err(e) if e.code() == NO_IDENTITY => {}
            Err(e) => eprintln!("Foxglove notifications: the arrival event: {}", e.message()),
        }
        let dir = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .ok_or("no LOCALAPPDATA")?
            .join(r"Microsoft\Windows\Notifications");
        super::super::screens_watch::folder(&dir, wake)
            .map(|w| Arrivals {
                by_event: false,
                _inner: Inner::Folder(w),
            })
            .ok_or_else(|| format!("can't watch {}", dir.display()))
    }

    fn app_name(n: &UserNotification) -> String {
        let info = n.AppInfo().ok();
        let display = info
            .as_ref()
            .and_then(|a| a.DisplayInfo().and_then(|d| d.DisplayName()).ok())
            .map(|s| s.to_string());
        let id = || {
            info.as_ref()
                .and_then(|a| a.AppUserModelId().ok())
                .map(|s| short_id(&s.to_string()))
        };
        display
            .filter(|s| !s.trim().is_empty())
            .or_else(id)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Windows".into())
    }

    /// An app id's readable end: `Microsoft.WindowsStore_8wekyb3d8bbwe!App` gives `WindowsStore`,
    /// a desktop app's `Contoso.Chat` gives `Chat`, a program's path its name.
    fn short_id(id: &str) -> String {
        let id = id.split('!').next().unwrap_or(id);
        let id = id.split('_').next().unwrap_or(id);
        let id = if id.to_lowercase().ends_with(".exe") {
            &id[..id.len() - 4]
        } else {
            id
        };
        id.rsplit(['.', '\\']).next().unwrap_or(id).to_string()
    }

    fn texts(n: &UserNotification) -> windows::core::Result<Vec<String>> {
        let binding = n
            .Notification()?
            .Visual()?
            .GetBinding(&KnownNotificationBindings::ToastGeneric()?)?;
        let elements = binding.GetTextElements()?;
        let mut out = Vec::new();
        for i in 0..elements.Size()? {
            out.push(elements.GetAt(i)?.Text()?.to_string());
        }
        Ok(out)
    }

    /// The toasts in the notification centre; an app's text is read only when `text_for` says.
    pub fn toasts(text_for: &dyn Fn(&str) -> bool) -> Result<Vec<Toast>, String> {
        let read = || -> windows::core::Result<Vec<Toast>> {
            let all = UserNotificationListener::Current()?
                .GetNotificationsAsync(NotificationKinds::Toast)?
                .join()?;
            let mut out = Vec::new();
            for i in 0..all.Size()? {
                let Ok(n) = all.GetAt(i) else { continue };
                let Ok(id) = n.Id() else { continue };
                let app = app_name(&n);
                let text = if text_for(&app) {
                    texts(&n).unwrap_or_default()
                } else {
                    Vec::new()
                };
                out.push(Toast { id, app, text });
            }
            Ok(out)
        };
        read().map_err(|e| e.message())
    }

    /// The shell's quiet-hours profile: 0 off, else Do not disturb / Focus assist is on.
    fn focus_assist() -> Option<u32> {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        type Query = unsafe extern "system" fn(
            *const u64,
            *const core::ffi::c_void,
            *const core::ffi::c_void,
            *mut u32,
            *mut core::ffi::c_void,
            *mut u32,
        ) -> i32;
        // WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED
        const STATE: u64 = 0x0D83_063E_A3BF_1C75;
        // SAFETY: ntdll is mapped in every process, so its handle needs no release. The pointer
        // GetProcAddress returns is NtQueryWnfStateData, whose signature `Query` matches. Every
        // pointer passed is to a live local; the buffer is 4 bytes and `size` says so, and the
        // value is used only when the call succeeded and wrote exactly 4 bytes.
        unsafe {
            let ntdll = GetModuleHandleW(w!("ntdll.dll")).ok()?;
            let f = GetProcAddress(ntdll, s!("NtQueryWnfStateData"))?;
            let query: Query = std::mem::transmute(f);
            let (mut stamp, mut value, mut size) = (0u32, 0u32, 4u32);
            let status = query(
                &STATE,
                std::ptr::null(),
                std::ptr::null(),
                &mut stamp,
                (&mut value as *mut u32).cast(),
                &mut size,
            );
            (status == 0 && size == 4).then_some(value)
        }
    }

    pub fn quiet() -> Option<Quiet> {
        use windows::Win32::UI::Shell::*;
        if focus_assist().is_some_and(|v| v != 0) {
            return Some(Quiet::DoNotDisturb);
        }
        // SAFETY: takes no pointers and only reads the shell's state.
        match unsafe { SHQueryUserNotificationState() }.ok()? {
            QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_APP => Some(Quiet::FullScreen),
            QUNS_PRESENTATION_MODE => Some(Quiet::Presentation),
            QUNS_QUIET_TIME => Some(Quiet::NewUser),
            QUNS_NOT_PRESENT => Some(Quiet::NotPresent),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn app_ids_shorten_to_their_name() {
            assert_eq!(
                super::short_id("Microsoft.WindowsStore_8wekyb3d8bbwe!App"),
                "WindowsStore"
            );
            assert_eq!(super::short_id("Contoso.Chat"), "Chat");
            assert_eq!(super::short_id(r"C:\Tools\thing.exe"), "thing");
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub struct Inner;

    pub fn access() -> Access {
        Access::Unavailable("not on Windows".into())
    }
    pub fn request_access() -> Access {
        access()
    }
    pub fn watch(_: Arc<dyn Fn() + Send + Sync>) -> Result<Arrivals, String> {
        Err("not on Windows".into())
    }
    pub fn toasts(_: &dyn Fn(&str) -> bool) -> Result<Vec<Toast>, String> {
        Ok(Vec::new())
    }
    pub fn quiet() -> Option<Quiet> {
        None
    }
}
