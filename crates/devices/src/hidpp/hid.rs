//! The hidapi side of HID++ (the same stack as `pc_lighting::hid`): one device's long-report
//! collection, and its short twin when it has one, opened with shared access (Logitech's own
//! app keeps its handles) for one read of its batteries and closed again.
//!
//! On Windows each top-level collection is its own handle: a long request goes to the long
//! collection, but a receiver answers some of them (HID++ 1.0 errors) with a short report, which
//! arrives on the short collection. [`HidTransport::read`] waits on both, in short slices, only
//! while a request waits for its answer (bounded by the request's own deadline).

use std::ffi::CString;
use std::time::{Duration, Instant};

use hidapi::{HidApi, HidDevice};

use super::probe::Candidate;
use super::{Transport, LONG_LEN, REPORT_SHORT};
use crate::{DeviceError, Result};

/// One slice of a wait on a device with two collections.
const SLICE: Duration = Duration::from_millis(10);

pub struct HidTransport {
    long: HidDevice,
    short: Option<HidDevice>,
}

impl HidTransport {
    /// Opens a candidate's collections. A short twin that won't open is left out.
    pub fn open(c: &Candidate) -> Result<Self> {
        let api = HidApi::new().map_err(|e| DeviceError::Backend(e.to_string()))?;
        let open = |path: &str| -> Result<HidDevice> {
            let path =
                CString::new(path).map_err(|_| DeviceError::Backend("bad device path".into()))?;
            api.open_path(&path).map_err(|_| DeviceError::Disconnected)
        };
        Ok(Self {
            long: open(&c.long)?,
            short: c.short.as_deref().and_then(|p| open(p).ok()),
        })
    }
}

fn read_one(dev: &HidDevice, timeout: Duration) -> Result<Option<Vec<u8>>> {
    let mut buf = [0u8; LONG_LEN + 1];
    let ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    match dev.read_timeout(&mut buf, ms) {
        Ok(0) => Ok(None),
        Ok(n) => Ok(Some(buf[..n].to_vec())),
        Err(_) => Err(DeviceError::Disconnected),
    }
}

impl Transport for HidTransport {
    fn write(&mut self, report: &[u8]) -> Result<()> {
        let dev = match (&self.short, report.first()) {
            (Some(short), Some(&REPORT_SHORT)) => short,
            _ => &self.long,
        };
        let n = dev.write(report).map_err(|_| DeviceError::Disconnected)?;
        if n < report.len() {
            return Err(DeviceError::Backend(format!(
                "wrote {n} of {} bytes",
                report.len()
            )));
        }
        Ok(())
    }

    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        let Some(short) = &self.short else {
            return read_one(&self.long, timeout);
        };
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let slice = left.min(SLICE);
            if let Some(r) = read_one(&self.long, slice)? {
                return Ok(Some(r));
            }
            if let Some(r) = read_one(short, Duration::ZERO)? {
                return Ok(Some(r));
            }
            if left.is_zero() {
                return Ok(None);
            }
        }
    }
}
