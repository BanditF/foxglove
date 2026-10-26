//! The iCUE SDK v4, loaded from the user's own copy of Corsair's DLL (we ship none of Corsair's
//! files). Our own declarations of its C interface; only what mirroring needs: connect, list
//! devices, read LED colours, and the session and device-connection events. Nothing here
//! sets colours or requests control.

/// What iCUE says, from its own threads.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// The SDK session connected (true) or isn't (closed, lost, refused, timed out).
    Session(bool),
    /// A Corsair device connected or disconnected.
    Device { id: String, connected: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: String,
    pub model: String,
    pub leds: i32,
}

/// What the plugin needs from the SDK; the real one is [`Icue`], tests use a fake.
pub trait Sdk {
    fn devices(&self) -> Result<Vec<Device>, String>;
    /// The device's LED ids.
    fn leds(&self, device: &str) -> Result<Vec<u32>, String>;
    /// The current colours of those LEDs: (r, g, b, alpha).
    fn colors(&self, device: &str, leds: &[u32]) -> Result<Vec<(u8, u8, u8, u8)>, String>;
}

/// A device id or name from the SDK's fixed, NUL-terminated byte arrays.
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())])
        .into_owned()
}

#[cfg(windows)]
pub use real::Icue;

#[cfg(windows)]
mod real {
    use std::ffi::c_void;
    use std::sync::mpsc::Sender;

    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    use super::{text, Device, Notice, Sdk};

    const STRING_M: usize = 128;
    const DEVICE_COUNT_MAX: usize = 64;
    const LED_COUNT_MAX: usize = 512;
    const SESSION_CONNECTED: i32 = 6;
    const EVENT_DEVICE_CONNECTION: i32 = 1;

    #[repr(C)]
    struct Version {
        major: i32,
        minor: i32,
        patch: i32,
    }

    #[repr(C)]
    struct SessionStateChanged {
        state: i32,
        client: Version,
        server: Version,
        host: Version,
    }

    #[repr(C)]
    struct DeviceInfo {
        kind: i32,
        id: [u8; STRING_M],
        serial: [u8; STRING_M],
        model: [u8; STRING_M],
        led_count: i32,
        channel_count: i32,
    }

    #[repr(C)]
    struct DeviceFilter {
        device_type_mask: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LedPosition {
        id: u32,
        cx: f64,
        cy: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LedColor {
        id: u32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    }

    #[repr(C)]
    struct DeviceConnectionStatus {
        device_id: [u8; STRING_M],
        is_connected: bool,
    }

    #[repr(C)]
    struct Event {
        id: i32,
        data: *const c_void,
    }

    /// What GetProcAddress returns.
    type Proc = unsafe extern "system" fn() -> isize;
    type SessionHandler = unsafe extern "C" fn(*mut c_void, *const SessionStateChanged);
    type EventHandler = unsafe extern "C" fn(*mut c_void, *const Event);
    type Connect = unsafe extern "C" fn(SessionHandler, *mut c_void) -> i32;
    type Subscribe = unsafe extern "C" fn(EventHandler, *mut c_void) -> i32;
    type GetDevices =
        unsafe extern "C" fn(*const DeviceFilter, i32, *mut DeviceInfo, *mut i32) -> i32;
    type GetLedPositions = unsafe extern "C" fn(*const u8, i32, *mut LedPosition, *mut i32) -> i32;
    type GetLedColors = unsafe extern "C" fn(*const u8, i32, *mut LedColor) -> i32;

    pub struct Icue {
        _dll: HMODULE,
        get_devices: GetDevices,
        get_led_positions: GetLedPositions,
        get_led_colors: GetLedColors,
    }

    // The SDK's functions may be called from any thread.
    unsafe impl Send for Icue {}

    unsafe extern "C" fn on_session(context: *mut c_void, e: *const SessionStateChanged) {
        if let (Some(tx), Some(e)) = (
            unsafe { (context as *const Sender<Notice>).as_ref() },
            unsafe { e.as_ref() },
        ) {
            let _ = tx.send(Notice::Session(e.state == SESSION_CONNECTED));
        }
    }

    unsafe extern "C" fn on_event(context: *mut c_void, e: *const Event) {
        let (Some(tx), Some(e)) = (
            unsafe { (context as *const Sender<Notice>).as_ref() },
            unsafe { e.as_ref() },
        ) else {
            return;
        };
        if e.id != EVENT_DEVICE_CONNECTION {
            return;
        }
        if let Some(d) = unsafe { (e.data as *const DeviceConnectionStatus).as_ref() } {
            let _ = tx.send(Notice::Device {
                id: text(&d.device_id),
                connected: d.is_connected,
            });
        }
    }

    fn device_id(id: &str) -> [u8; STRING_M] {
        let mut b = [0u8; STRING_M];
        let n = id.len().min(STRING_M - 1);
        b[..n].copy_from_slice(&id.as_bytes()[..n]);
        b
    }

    impl Icue {
        /// Loads the DLL and connects; session and device events go to `tx` from then on.
        pub fn load(path: &str, tx: Sender<Notice>) -> Result<Icue, String> {
            let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
            let dll = unsafe { LoadLibraryW(wide.as_ptr()) };
            if dll.is_null() {
                return Err(format!(
                    "can't load {path}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let get = |name: &[u8]| {
                unsafe { GetProcAddress(dll, name.as_ptr()) }.ok_or_else(|| {
                    format!(
                        "{path} has no {}",
                        String::from_utf8_lossy(&name[..name.len() - 1])
                    )
                })
            };
            // SAFETY: each pointer comes from the SDK's export of that name, whose C signature
            // the type above declares (iCUESDK.h, v4).
            let connect =
                unsafe { std::mem::transmute::<Proc, Connect>(get(b"CorsairConnect\0")?) };
            let subscribe = unsafe {
                std::mem::transmute::<Proc, Subscribe>(get(b"CorsairSubscribeForEvents\0")?)
            };
            let icue = Icue {
                _dll: dll,
                get_devices: unsafe {
                    std::mem::transmute::<Proc, GetDevices>(get(b"CorsairGetDevices\0")?)
                },
                get_led_positions: unsafe {
                    std::mem::transmute::<Proc, GetLedPositions>(get(b"CorsairGetLedPositions\0")?)
                },
                get_led_colors: unsafe {
                    std::mem::transmute::<Proc, GetLedColors>(get(b"CorsairGetLedColors\0")?)
                },
            };
            // The context lives as long as the process: the SDK may call back at any time.
            let context = Box::into_raw(Box::new(tx)) as *mut c_void;
            let e = unsafe { connect(on_session, context) };
            if e != 0 {
                return Err(format!("CorsairConnect failed ({e})"));
            }
            let e = unsafe { subscribe(on_event, context) };
            if e != 0 {
                eprintln!("icue-mirror: no device events ({e}); a device change waits for the next session");
            }
            Ok(icue)
        }
    }

    impl Sdk for Icue {
        fn devices(&self) -> Result<Vec<Device>, String> {
            let filter = DeviceFilter {
                device_type_mask: -1,
            };
            let mut out: Vec<DeviceInfo> = (0..DEVICE_COUNT_MAX)
                .map(|_| DeviceInfo {
                    kind: 0,
                    id: [0; STRING_M],
                    serial: [0; STRING_M],
                    model: [0; STRING_M],
                    led_count: 0,
                    channel_count: 0,
                })
                .collect();
            let mut n = 0i32;
            let e = unsafe {
                (self.get_devices)(&filter, DEVICE_COUNT_MAX as i32, out.as_mut_ptr(), &mut n)
            };
            if e != 0 {
                return Err(format!("CorsairGetDevices failed ({e})"));
            }
            Ok(out
                .iter()
                .take(n.max(0) as usize)
                .map(|d| Device {
                    id: text(&d.id),
                    model: text(&d.model),
                    leds: d.led_count,
                })
                .collect())
        }

        fn leds(&self, device: &str) -> Result<Vec<u32>, String> {
            let id = device_id(device);
            let mut out = vec![
                LedPosition {
                    id: 0,
                    cx: 0.0,
                    cy: 0.0
                };
                LED_COUNT_MAX
            ];
            let mut n = 0i32;
            let e = unsafe {
                (self.get_led_positions)(
                    id.as_ptr(),
                    LED_COUNT_MAX as i32,
                    out.as_mut_ptr(),
                    &mut n,
                )
            };
            if e != 0 {
                return Err(format!("CorsairGetLedPositions failed ({e})"));
            }
            Ok(out.iter().take(n.max(0) as usize).map(|p| p.id).collect())
        }

        fn colors(&self, device: &str, leds: &[u32]) -> Result<Vec<(u8, u8, u8, u8)>, String> {
            let id = device_id(device);
            let mut out: Vec<LedColor> = leds
                .iter()
                .map(|l| LedColor {
                    id: *l,
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 0,
                })
                .collect();
            let e =
                unsafe { (self.get_led_colors)(id.as_ptr(), out.len() as i32, out.as_mut_ptr()) };
            if e != 0 {
                return Err(format!("CorsairGetLedColors failed ({e})"));
            }
            Ok(out.iter().map(|c| (c.r, c.g, c.b, c.a)).collect())
        }
    }
}

/// Elsewhere the SDK isn't loaded (the plugin is Windows-only for now).
#[cfg(not(windows))]
pub struct Icue;

#[cfg(not(windows))]
impl Icue {
    pub fn load(_path: &str, _tx: std::sync::mpsc::Sender<Notice>) -> Result<Icue, String> {
        Err("the iCUE SDK is loaded on Windows only".into())
    }
}

#[cfg(not(windows))]
impl Sdk for Icue {
    fn devices(&self) -> Result<Vec<Device>, String> {
        Ok(vec![])
    }
    fn leds(&self, _: &str) -> Result<Vec<u32>, String> {
        Ok(vec![])
    }
    fn colors(&self, _: &str, _: &[u32]) -> Result<Vec<(u8, u8, u8, u8)>, String> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn device_strings_stop_at_the_nul() {
        let mut b = [0u8; 16];
        b[..4].copy_from_slice(b"K70\0");
        assert_eq!(super::text(&b), "K70");
        assert_eq!(super::text(b"full"), "full");
    }
}
