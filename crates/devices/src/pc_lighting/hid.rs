//! The hidapi side: one enumeration for [`super::discover`], and a writer that opens a device
//! only while it sends (shared access, so the maker's app can keep its own handle).

use std::ffi::CString;

use hidapi::HidApi;

use super::{HidEntry, HidWriter};
use crate::{DeviceError, Result};

/// Every HID interface present now. Call it on an arrival or removal event, not on a timer.
pub fn enumerate() -> Result<Vec<HidEntry>> {
    let api = HidApi::new().map_err(|e| DeviceError::Backend(e.to_string()))?;
    Ok(api
        .device_list()
        .map(|d| HidEntry {
            vid: d.vendor_id(),
            pid: d.product_id(),
            usage_page: d.usage_page(),
            usage: d.usage(),
            path: d.path().to_string_lossy().into_owned(),
            product: d.product_string().map(str::to_string),
            serial: d.serial_number().map(str::to_string),
        })
        .collect())
}

/// Writes reports to one HID path, opening it for each [`HidWriter::send`].
pub struct PathWriter {
    path: CString,
}

impl PathWriter {
    pub fn new(path: &str) -> Result<Self> {
        CString::new(path)
            .map(|path| Self { path })
            .map_err(|_| DeviceError::Backend("bad device path".into()))
    }
}

impl HidWriter for PathWriter {
    fn send(&mut self, reports: &[Vec<u8>]) -> Result<()> {
        let api = HidApi::new().map_err(|e| DeviceError::Backend(e.to_string()))?;
        let dev = api
            .open_path(&self.path)
            .map_err(|_| DeviceError::Disconnected)?;
        for report in reports {
            let n = dev
                .write(report)
                .map_err(|e| DeviceError::Backend(e.to_string()))?;
            if n < report.len() {
                return Err(DeviceError::Backend(format!(
                    "wrote {n} of {} bytes",
                    report.len()
                )));
            }
        }
        Ok(())
    }
}
