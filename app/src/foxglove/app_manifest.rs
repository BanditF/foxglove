//! The application manifest every Windows build embeds (#447): `packaging/foxglove.exe.manifest`,
//! put in by `build.rs`. Tests only: they read it back out of the running test binary, which
//! links the same resources as foxglove.exe.

#[cfg(test)]
mod tests {
    use windows::core::PCWSTR;
    use windows::Win32::System::LibraryLoader::{
        FindResourceW, LoadResource, LockResource, SizeofResource,
    };
    use windows::Win32::UI::HiDpi::{
        AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext,
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::WindowsAndMessaging::RT_MANIFEST;

    /// The embedded RT_MANIFEST resource #1 of this executable.
    fn embedded_manifest() -> String {
        // SAFETY: a null module means this executable; resource 1 is the process manifest (an
        // integer id passed as MAKEINTRESOURCE). The handles from Find/Load/LockResource stay
        // valid for the life of the module, and SizeofResource bounds the slice.
        unsafe {
            let info = FindResourceW(None, PCWSTR(1 as _), RT_MANIFEST);
            assert!(!info.is_invalid(), "no RT_MANIFEST resource embedded");
            let data = LoadResource(None, info).expect("LoadResource");
            let len = SizeofResource(None, info) as usize;
            let ptr = LockResource(data) as *const u8;
            assert!(!ptr.is_null() && len > 0);
            String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned()
        }
    }

    #[test]
    fn manifest_is_embedded() {
        let m = embedded_manifest();
        for needle in [
            "PerMonitorV2",
            "<longPathAware",
            "{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}",
            "Microsoft.Windows.Common-Controls",
        ] {
            assert!(m.contains(needle), "embedded manifest lacks {needle}");
        }
        assert!(!m.contains("FOXGLOVE_MSIX"), "the marker was not replaced");
        assert_eq!(m.contains("<msix"), crate::foxglove::identity::ENABLED);
    }

    #[test]
    fn process_starts_per_monitor_v2() {
        // SAFETY: plain queries of this thread's DPI context; no handles are kept.
        let v2 = unsafe {
            AreDpiAwarenessContextsEqual(
                GetThreadDpiAwarenessContext(),
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            )
        };
        assert!(v2.as_bool(), "the manifest's dpiAwareness did not apply");
    }
}
