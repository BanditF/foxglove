// SPDX-License-Identifier: MIT
//! The sign-in hand-over event (sign-in hand-over slice 3): one named kernel event per user
//! session, `Local\Hideout-SignIn-<user SID>`, created with a current-user-only ACL and the
//! default Medium label (no Low-integrity or AppContainer process can open it by name).
//!
//! Hideout sets it once it has decided about Foxglove's row; a waiting `foxglove.exe --tray`
//! wakes on it. It is a **hint, never trust**: any same-user process can set it, and setting it
//! only makes the waiter run its own verified check early. Nothing is written to disk; the object
//! is gone when the last handle closes. Manual-reset: once set it stays set for this session.

use std::io;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

use crate::{current_user_sid, wide, Descriptor};

/// The event's name for the SID `sid`.
pub fn name_for(sid: &str) -> String {
    format!(r"Local\Hideout-SignIn-{sid}")
}

/// An open handle on the event (created if it did not exist).
pub struct SignInEvent(HANDLE, bool);

// SAFETY: a kernel event handle may be used from any thread.
unsafe impl Send for SignInEvent {}
unsafe impl Sync for SignInEvent {}

impl SignInEvent {
    /// Opens the event, creating it (not signalled) when no process has yet.
    pub fn open() -> io::Result<SignInEvent> {
        Self::open_named(&name_for(&current_user_sid()?))
    }

    fn open_named(name: &str) -> io::Result<SignInEvent> {
        let sd = Descriptor::current_user_only()?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.as_ptr(),
            bInheritHandle: 0,
        };
        let w = wide(name);
        // SAFETY: `sa` and `w` outlive the call.
        let h = unsafe { CreateEventW(&sa, 1, 0, w.as_ptr()) };
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: read at once, on this thread, after the call that set it.
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        Ok(SignInEvent(h, existed))
    }

    /// Whether the event was there already when this handle opened it (another process made it
    /// first; the hub logs that).
    pub fn existed(&self) -> bool {
        self.1
    }

    /// Sets the event (it stays set).
    pub fn set(&self) -> io::Result<()> {
        // SAFETY: the handle is ours and open.
        if unsafe { SetEvent(self.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Waits up to `limit` for the event; true when it is set. One kernel wait, no polling.
    pub fn wait(&self, limit: Duration) -> bool {
        let ms = u32::try_from(limit.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: the handle is ours and open.
        unsafe { WaitForSingleObject(self.0, ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for SignInEvent {
    fn drop(&mut self) {
        // SAFETY: the handle is ours; closed once.
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique(tag: &str) -> String {
        format!(r"Local\Hideout-SignIn-test-{tag}-{}", std::process::id())
    }

    #[test]
    fn the_name_carries_the_sid_and_stays_in_the_session() {
        assert_eq!(name_for("S-1-5-21-1"), r"Local\Hideout-SignIn-S-1-5-21-1");
    }

    #[test]
    fn unset_times_out_and_set_is_seen_through_a_second_handle() {
        let n = unique("two");
        let a = SignInEvent::open_named(&n).unwrap();
        let b = SignInEvent::open_named(&n).unwrap();
        assert!(!b.wait(Duration::from_millis(20)));
        a.set().unwrap();
        assert!(b.wait(Duration::from_millis(20)));
        // Manual-reset: still set.
        assert!(b.wait(Duration::from_millis(0)));
    }

    #[test]
    fn the_event_is_gone_with_its_last_handle() {
        let n = unique("gone");
        {
            let a = SignInEvent::open_named(&n).unwrap();
            a.set().unwrap();
        }
        let again = SignInEvent::open_named(&n).unwrap();
        assert!(!again.wait(Duration::from_millis(0)));
    }
}
