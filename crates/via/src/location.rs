//! Where a keyboard is plugged in (#292), for keyboards whose USB serial can't tell them apart:
//! the USB device's container ID and location path (the port, through any hubs), read from
//! Windows' device tree once, when it's found. Steadier than the HID path, which also names the
//! interface; it still changes when the board moves to another port, which the app's "This is
//! the same keyboard" answers.

/// `usb:<container id>@<location path>` for a HID interface path, if Windows knows them.
#[cfg(windows)]
pub fn of(hid_path: &str) -> Option<String> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_PropertyW, CM_Get_Device_IDW, CM_Get_Device_Interface_PropertyW, CM_Get_Parent, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL,
        CR_SUCCESS,
    };
    use windows_sys::Win32::Devices::Properties::{DEVPKEY_Device_ContainerId, DEVPKEY_Device_InstanceId, DEVPKEY_Device_LocationPaths};
    use windows_sys::Win32::Foundation::DEVPROPKEY;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    /// A property's raw bytes.
    fn property(get: impl Fn(*mut u8, *mut u32) -> u32) -> Option<Vec<u8>> {
        let mut size = 0u32;
        get(std::ptr::null_mut(), &mut size);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        (get(buf.as_mut_ptr(), &mut size) == CR_SUCCESS).then_some(buf)
    }
    /// The first string of a UTF-16 string or string list.
    fn text(bytes: &[u8]) -> String {
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|u| *u != 0).collect();
        String::from_utf16_lossy(&units)
    }
    let node_property = |node: u32, key: &DEVPROPKEY| {
        property(|buf, size| {
            let mut kind = 0;
            unsafe { CM_Get_DevNode_PropertyW(node, key, &mut kind, buf, size, 0) }
        })
    };

    let path = wide(hid_path);
    let instance = property(|buf, size| {
        let mut kind = 0;
        unsafe { CM_Get_Device_Interface_PropertyW(path.as_ptr(), &DEVPKEY_Device_InstanceId, &mut kind, buf, size, 0) }
    })?;
    let instance = wide(&text(&instance));
    let mut node = 0u32;
    if unsafe { CM_Locate_DevNodeW(&mut node, instance.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) } != CR_SUCCESS {
        return None;
    }
    // Up from the HID interface to the USB device it belongs to.
    for _ in 0..4 {
        let mut id = [0u16; 512];
        if unsafe { CM_Get_Device_IDW(node, id.as_mut_ptr(), id.len() as u32, 0) } != CR_SUCCESS {
            return None;
        }
        let id = String::from_utf16_lossy(&id[..id.iter().position(|u| *u == 0).unwrap_or(id.len())]);
        if id.to_ascii_uppercase().starts_with("USB\\") && !id.to_ascii_uppercase().contains("&MI_") {
            let location = text(&node_property(node, &DEVPKEY_Device_LocationPaths)?);
            let container = node_property(node, &DEVPKEY_Device_ContainerId)
                .filter(|b| b.len() == 16)
                .map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>())
                .unwrap_or_default();
            return (!location.is_empty()).then(|| format!("usb:{container}@{location}"));
        }
        let mut parent = 0u32;
        if unsafe { CM_Get_Parent(&mut parent, node, 0) } != CR_SUCCESS {
            return None;
        }
        node = parent;
    }
    None
}

#[cfg(not(windows))]
pub fn of(_hid_path: &str) -> Option<String> {
    None
}

#[cfg(all(test, windows, feature = "hid"))]
mod tests {
    /// Reads where this PC's HID devices are plugged in (properties only, no device I/O). Run by
    /// hand: `cargo test -p foxglove-via -- --ignored locates`.
    #[test]
    #[ignore]
    fn locates_connected_hid_devices() {
        let api = hidapi::HidApi::new().unwrap();
        let found: Vec<String> = api.device_list().filter_map(|d| super::of(&d.path().to_string_lossy())).collect();
        println!("{found:#?}");
        assert!(found.iter().all(|f| f.starts_with("usb:") && f.contains('@')));
        assert!(!found.is_empty(), "no USB HID device located");
    }
}
