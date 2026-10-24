//! Keyboard interface arrival/removal notifications.

use crate::ui_wake::UiWake;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Register_Notification, CM_Unregister_Notification, CM_NOTIFY_ACTION,
    CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL, CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL,
    CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER, CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, CR_SUCCESS,
    HCMNOTIFICATION,
};

struct Context {
    changed: AtomicBool,
    identity: AtomicU32,
    wake: UiWake,
}

pub(crate) struct DeviceWatch {
    handle: HCMNOTIFICATION,
    context: Box<Context>,
}

impl DeviceWatch {
    pub(crate) fn new(wake: UiWake, hid: bool) -> Option<Self> {
        let context = Box::new(Context {
            changed: AtomicBool::new(false),
            identity: AtomicU32::new(0),
            wake,
        });
        let mut filter = CM_NOTIFY_FILTER {
            cbSize: std::mem::size_of::<CM_NOTIFY_FILTER>() as u32,
            FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
            ..Default::default()
        };
        filter.u.DeviceInterface.ClassGuid = windows::core::GUID::from_u128(if hid {
            0x4d1e55b2_f16f_11cf_88cb_001111000030
        } else {
            0x86e0d1e0_8089_11d0_9ce4_08003e301f73
        });
        let mut handle = HCMNOTIFICATION::default();
        // The boxed context stays at this address until unregister has drained callbacks.
        let result = unsafe {
            CM_Register_Notification(
                &filter,
                Some((&*context as *const Context).cast()),
                Some(on_notification),
                &mut handle,
            )
        };
        if result != CR_SUCCESS {
            return None;
        }
        Some(Self { handle, context })
    }

    pub(crate) fn set_identity(&self, identity: Option<(u16, u16)>) {
        let value = identity.map_or(0, |(vid, pid)| (u32::from(vid) << 16) | u32::from(pid));
        self.context.identity.store(value, Ordering::Relaxed);
    }

    pub(crate) fn take_change(&self) -> bool {
        self.context.changed.swap(false, Ordering::Relaxed)
    }
}

impl Drop for DeviceWatch {
    fn drop(&mut self) {
        // Unregister waits for callbacks before the context is freed.
        unsafe { CM_Unregister_Notification(self.handle) };
    }
}

unsafe extern "system" fn on_notification(
    _handle: HCMNOTIFICATION,
    context: *const core::ffi::c_void,
    action: CM_NOTIFY_ACTION,
    event: *const CM_NOTIFY_EVENT_DATA,
    size: u32,
) -> u32 {
    if action == CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL
        || action == CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL
    {
        // Registered with a stable Box, kept alive until unregister returns.
        let context = unsafe { &*context.cast::<Context>() };
        let identity = context.identity.load(Ordering::Relaxed);
        if identity == 0 || event.is_null() {
            return 0;
        }
        // SymbolicLink is a variable-length UTF-16 tail, bounded by the callback's size.
        let path =
            unsafe { std::ptr::addr_of!((*event).u.DeviceInterface.SymbolicLink).cast::<u16>() };
        let offset = path as usize - event as usize;
        if (size as usize) < offset {
            return 0;
        }
        let path = unsafe { std::slice::from_raw_parts(path, (size as usize - offset) / 2) };
        let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
        if !super::connection::matches_interface(
            &String::from_utf16_lossy(&path[..end]),
            (identity >> 16) as u16,
            identity as u16,
        ) {
            return 0;
        }
        context.changed.store(true, Ordering::Relaxed);
        context.wake.request_repaint();
    }
    0
}
