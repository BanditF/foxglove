//! Cancellable overlapped Raw HID input. hidapi does not expose its read event or cancellation.

use std::ffi::CStr;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use anyhow::{ensure, Result};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetPreparsedData, HidD_SetNumInputBuffers, HidP_GetCaps,
    HIDP_CAPS, HIDP_STATUS_SUCCESS,
};
use windows_sys::Win32::Foundation::{
    ERROR_IO_PENDING, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, SetEvent, WaitForMultipleObjects, WaitForSingleObject, INFINITE,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

use crate::link::Report;

pub(crate) struct Input {
    device: OwnedHandle,
    stop: OwnedHandle,
    length: usize,
}

fn event() -> Result<OwnedHandle> {
    // SAFETY: no name or security attributes; ownership of the new handle is transferred once.
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    ensure!(
        !handle.is_null(),
        "create HID event: {}",
        io::Error::last_os_error()
    );
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

impl Input {
    pub(crate) fn open(path: &CStr) -> Result<Self> {
        let path: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: path is terminated; no borrowed handle or pointer outlives this call.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        ensure!(
            handle != INVALID_HANDLE_VALUE,
            "open HID input: {}",
            io::Error::last_os_error()
        );
        let device = unsafe { OwnedHandle::from_raw_handle(handle) };
        // SAFETY: the preparsed data belongs to this device and is freed after querying caps.
        let length = unsafe {
            let mut data = 0;
            ensure!(
                HidD_GetPreparsedData(handle, &mut data),
                "HID input descriptor: {}",
                io::Error::last_os_error()
            );
            let mut caps: HIDP_CAPS = std::mem::zeroed();
            let status = HidP_GetCaps(data, &mut caps);
            HidD_FreePreparsedData(data);
            ensure!(
                status == HIDP_STATUS_SUCCESS,
                "HID input capabilities: {status}"
            );
            ensure!(caps.InputReportByteLength > 0, "HID has no input reports");
            // Match hidapi's queue size; failure leaves the driver's default intact.
            HidD_SetNumInputBuffers(handle, 64);
            caps.InputReportByteLength as usize
        };
        Ok(Self {
            device,
            stop: event()?,
            length,
        })
    }

    pub(crate) fn interrupt(&self) {
        // SAFETY: stop is a live manual-reset event and remains signalled for future reads.
        unsafe {
            SetEvent(self.stop.as_raw_handle());
        }
    }

    pub(crate) fn read(&self, buf: &mut Report) -> Result<usize> {
        // Each operation owns its buffer, OVERLAPPED and event until completion, including
        // cancellation. Only the link reader calls this, and handles outlive its join.
        let ready = event()?;
        let mut bytes = vec![0; self.length];
        let mut count = 0;
        let handle = self.device.as_raw_handle();
        let stop = self.stop.as_raw_handle();
        // SAFETY: all handles are owned, both buffers stay alive through GetOverlappedResult.
        unsafe {
            if WaitForSingleObject(stop, 0) == WAIT_OBJECT_0 {
                return Ok(0);
            }
            let mut overlapped: OVERLAPPED = std::mem::zeroed();
            overlapped.hEvent = ready.as_raw_handle();
            let started = ReadFile(
                handle,
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                &mut count,
                &mut overlapped,
            );
            if started == 0 {
                let error = io::Error::last_os_error();
                ensure!(
                    error.raw_os_error() == Some(ERROR_IO_PENDING as i32),
                    "HID read: {error}"
                );
                let handles: [HANDLE; 2] = [stop, ready.as_raw_handle()];
                let wait = WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE);
                if wait != WAIT_OBJECT_0 + 1 {
                    // CancelIoEx alone is not enough: finish cancellation before freeing memory.
                    let error = io::Error::last_os_error();
                    CancelIoEx(handle, &overlapped);
                    GetOverlappedResult(handle, &overlapped, &mut count, 1);
                    ensure!(wait == WAIT_OBJECT_0, "wait for HID input: {error}");
                    return Ok(0);
                }
                ensure!(
                    GetOverlappedResult(handle, &overlapped, &mut count, 0) != 0,
                    "HID read completion: {}",
                    io::Error::last_os_error()
                );
            }
        }
        let bytes = &bytes[..count as usize];
        // Windows includes a zero report ID for unnumbered reports, just as in hidapi.
        let bytes = bytes.strip_prefix(&[0]).unwrap_or(bytes);
        let n = bytes.len().min(buf.len());
        buf.fill(0);
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::GENERIC_WRITE;
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_INBOUND;
    use windows_sys::Win32::System::Pipes::CreateNamedPipeW;

    #[test]
    fn interrupt_is_latched_before_read_starts() {
        let input = Input {
            device: event().unwrap(),
            stop: event().unwrap(),
            length: 33,
        };
        input.interrupt();
        assert_eq!(input.read(&mut [0; 32]).unwrap(), 0);
        assert_eq!(input.read(&mut [0; 32]).unwrap(), 0);
    }

    #[test]
    fn interrupt_cancels_and_finishes_a_pending_read() {
        let name: Vec<u16> = format!(r"\\.\pipe\digitalis-read-test-{}", std::process::id())
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: the terminated pipe name and all handles live through the reader join.
        let (device, _writer) = unsafe {
            let server = CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED,
                0,
                1,
                64,
                64,
                0,
                std::ptr::null(),
            );
            assert_ne!(server, INVALID_HANDLE_VALUE);
            let server = OwnedHandle::from_raw_handle(server);
            let writer = CreateFileW(
                name.as_ptr(),
                GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            assert_ne!(writer, INVALID_HANDLE_VALUE);
            (server, OwnedHandle::from_raw_handle(writer))
        };
        let input = Arc::new(Input {
            device,
            stop: event().unwrap(),
            length: 33,
        });
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn({
            let input = input.clone();
            move || tx.send(input.read(&mut [0; 32]).unwrap()).unwrap()
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        input.interrupt();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
        reader.join().unwrap();
    }
}
