// SPDX-License-Identifier: MIT
//! What kind of disk a drive letter is on, for the "local fixed drive" check (trust model §4:
//! no removable disk). `GetDriveTypeW` alone says `DRIVE_FIXED` for a USB SSD or HDD, so the
//! storage bus type is asked too, and only an internal bus passes. Everything else, including a
//! bus type that can't be read, fails closed.
//!
//! The questions sit behind [`DriveProbe`] so tests use a fake; [`SystemDrives`] is the real one.

/// A storage bus (`STORAGE_BUS_TYPE`, winioctl.h). Anything not named is `Other(raw)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusType {
    Unknown,
    Scsi,
    Atapi,
    Ata,
    Ieee1394,
    Raid,
    Sas,
    Sata,
    Usb,
    Sd,
    Mmc,
    Virtual,
    FileBackedVirtual,
    Nvme,
    Other(i32),
}

impl BusType {
    pub fn from_raw(raw: i32) -> BusType {
        match raw {
            0 => BusType::Unknown,
            1 => BusType::Scsi,
            2 => BusType::Atapi,
            3 => BusType::Ata,
            4 => BusType::Ieee1394,
            7 => BusType::Usb,
            8 => BusType::Raid,
            10 => BusType::Sas,
            11 => BusType::Sata,
            12 => BusType::Sd,
            13 => BusType::Mmc,
            14 => BusType::Virtual,
            15 => BusType::FileBackedVirtual,
            17 => BusType::Nvme,
            n => BusType::Other(n),
        }
    }

    /// Whether disks on this bus count as this PC's own internal disks. An allow-list: a
    /// removable (USB, 1394, SD, MMC), virtual (VHD), unknown or future bus is refused.
    pub fn is_internal(self) -> bool {
        matches!(
            self,
            BusType::Ata
                | BusType::Sata
                | BusType::Nvme
                | BusType::Scsi
                | BusType::Sas
                | BusType::Raid
        )
    }
}

/// The two questions the local-drive check asks about a drive root such as `C:\`.
pub trait DriveProbe {
    /// Windows reports the drive as fixed (`DRIVE_FIXED`): not network, removable, optical or RAM.
    fn is_fixed(&self, root: &str) -> bool;
    /// The bus the drive's disk is on, or None when it can't be read.
    fn bus_type(&self, root: &str) -> Option<BusType>;
}

/// A fixed drive on an internal bus; an unreadable bus type is not internal.
pub fn is_internal_fixed(probe: &dyn DriveProbe, root: &str) -> bool {
    probe.is_fixed(root) && probe.bus_type(root).is_some_and(BusType::is_internal)
}

/// The real answers from Windows.
pub struct SystemDrives;

#[cfg(windows)]
impl DriveProbe for SystemDrives {
    fn is_fixed(&self, root: &str) -> bool {
        // DRIVE_FIXED (winbase.h).
        const DRIVE_FIXED: u32 = 3;
        let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
        unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(wide.as_ptr()) == DRIVE_FIXED
        }
    }

    fn bus_type(&self, root: &str) -> Option<BusType> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        use windows_sys::Win32::System::Ioctl::{
            PropertyStandardQuery, StorageDeviceProperty, IOCTL_STORAGE_QUERY_PROPERTY,
            STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
        };
        use windows_sys::Win32::System::IO::DeviceIoControl;

        // `C:\` -> `\\.\C:` (the volume). Access 0 asks for metadata only: no admin needed.
        let letter = root.chars().next().filter(char::is_ascii_alphabetic)?;
        let dev: Vec<u16> = format!(r"\\.\{letter}:")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: `dev` is NUL-terminated and outlives the call; the handle is closed below.
        let h = unsafe {
            CreateFileW(
                dev.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        // SAFETY: the structs are plain data; `out` is the output buffer and its size is passed;
        // a zeroed query with these two ids asks for the standard device property.
        let bus = unsafe {
            let mut query: STORAGE_PROPERTY_QUERY = std::mem::zeroed();
            query.PropertyId = StorageDeviceProperty;
            query.QueryType = PropertyStandardQuery;
            // The descriptor is followed by variable strings; the fixed part holds the bus type.
            let mut out = [0u8; 1024];
            let mut got = 0u32;
            let ok = DeviceIoControl(
                h,
                IOCTL_STORAGE_QUERY_PROPERTY,
                std::ptr::addr_of!(query).cast(),
                std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                out.as_mut_ptr().cast(),
                out.len() as u32,
                &mut got,
                std::ptr::null_mut(),
            );
            let need = std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>();
            if ok != 0 && got as usize >= need {
                let d = std::ptr::read_unaligned(out.as_ptr().cast::<STORAGE_DEVICE_DESCRIPTOR>());
                Some(BusType::from_raw(d.BusType))
            } else {
                None
            }
        };
        // SAFETY: `h` is a handle this function opened and has not closed.
        unsafe { CloseHandle(h) };
        bus
    }
}

#[cfg(not(windows))]
impl DriveProbe for SystemDrives {
    fn is_fixed(&self, _: &str) -> bool {
        false
    }
    fn bus_type(&self, _: &str) -> Option<BusType> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers for one drive letter only (`'E'`, say); any other drive is not fixed and has no
    /// readable bus, so a check that asks about the wrong root fails.
    struct Fake(bool, Option<BusType>, char);
    impl Fake {
        fn on(letter: char, fixed: bool, bus: Option<BusType>) -> Fake {
            Fake(fixed, bus, letter)
        }
        fn mine(&self, root: &str) -> bool {
            root.chars()
                .next()
                .is_some_and(|c| c.eq_ignore_ascii_case(&self.2))
        }
    }
    impl DriveProbe for Fake {
        fn is_fixed(&self, root: &str) -> bool {
            self.mine(root) && self.0
        }
        fn bus_type(&self, root: &str) -> Option<BusType> {
            self.mine(root).then_some(self.1).flatten()
        }
    }

    #[test]
    fn a_usb_disk_that_reports_fixed_is_refused() {
        assert!(!is_internal_fixed(
            &Fake::on('E', true, Some(BusType::Usb)),
            r"E:\"
        ));
    }

    #[test]
    fn sata_nvme_and_the_other_internal_buses_are_accepted() {
        for b in [
            BusType::Sata,
            BusType::Nvme,
            BusType::Ata,
            BusType::Scsi,
            BusType::Sas,
            BusType::Raid,
        ] {
            assert!(
                is_internal_fixed(&Fake::on('C', true, Some(b)), r"C:\"),
                "{b:?}"
            );
        }
    }

    #[test]
    fn removable_virtual_unknown_and_future_buses_are_refused() {
        for b in [
            BusType::Ieee1394,
            BusType::Sd,
            BusType::Mmc,
            BusType::Virtual,
            BusType::FileBackedVirtual,
            BusType::Atapi,
            BusType::Unknown,
            BusType::from_raw(99),
        ] {
            assert!(
                !is_internal_fixed(&Fake::on('E', true, Some(b)), r"E:\"),
                "{b:?}"
            );
        }
    }

    #[test]
    fn a_bus_type_that_cannot_be_read_is_refused() {
        assert!(!is_internal_fixed(&Fake::on('C', true, None), r"C:\"));
    }

    #[test]
    fn a_drive_that_is_not_fixed_is_refused_whatever_the_bus() {
        assert!(!is_internal_fixed(
            &Fake::on('C', false, Some(BusType::Sata)),
            r"C:\"
        ));
    }

    #[test]
    fn the_path_check_uses_the_bus_for_the_drive_in_the_path() {
        use crate::permission::on_local_fixed_drive_with as local;
        assert!(!local(
            r"E:\dev\app.exe",
            &Fake::on('E', true, Some(BusType::Usb))
        ));
        assert!(local(
            r"C:\dev\app.exe",
            &Fake::on('C', true, Some(BusType::Nvme))
        ));
        assert!(!local(r"C:\dev\app.exe", &Fake::on('C', true, None)));
        // The probe is asked about the drive in the path: C: is internal, E: (USB) is not.
        let two = Fake::on('C', true, Some(BusType::Nvme));
        assert!(local(r"C:\dev\app.exe", &two));
        assert!(!local(r"E:\dev\app.exe", &two));
        // A UNC path is refused before the probe is asked, whatever it would say.
        let any = Fake::on('S', true, Some(BusType::Sata));
        assert!(!local(r"\srv\share\a.exe", &any));
        assert!(!local(r"\?\UNC\srv\share\a.exe", &any));
    }

    #[test]
    fn raw_values_map_as_winioctl_defines_them() {
        assert_eq!(BusType::from_raw(7), BusType::Usb);
        assert_eq!(BusType::from_raw(11), BusType::Sata);
        assert_eq!(BusType::from_raw(17), BusType::Nvme);
    }

    #[cfg(windows)]
    #[test]
    fn the_real_probe_reads_a_bus_for_the_temp_drive() {
        let here = crate::keyboard::canonical_path(&std::env::temp_dir()).unwrap();
        let root = format!("{}\\", &here[..2]);
        assert!(SystemDrives.is_fixed(&root));
        eprintln!("bus of {root}: {:?}", SystemDrives.bus_type(&root));
    }
}
