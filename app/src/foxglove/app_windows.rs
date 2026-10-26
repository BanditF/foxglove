//! Tells plugins when an app they wait for opens or closes its windows (`apps.watch`, #277), so
//! they don't poll for it. An out-of-context WinEvent hook (no admin, no injected code) reports
//! windows being shown and destroyed; a shown window's program is looked up only for visible
//! top-level windows. The windows already open are listed once, when a plugin starts watching.
//! Nothing runs on a timer. The hook is installed only while some plugin waits for an app
//! (#410): it hears every window shown or destroyed on the desktop, so with no watch it would
//! wake Foxglove for nothing.
//!
//! An elevated app's windows may not be reported (Windows hides them from a normal process).

pub fn start() {
    if !super::repaint_trace::disabled("apps-watch") {
        imp::start();
    }
}

/// The file name of a program's path, lower case: `C:\...\OBS64.exe` → `obs64.exe`.
fn exe_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_ascii_lowercase()
}

#[cfg(windows)]
mod imp {
    use windows::core::{BOOL, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetAncestor, GetMessageW, GetWindowThreadProcessId, IsWindowVisible, PostThreadMessageW, CHILDID_SELF, EVENT_OBJECT_DESTROY,
        EVENT_OBJECT_SHOW, GA_ROOT, MSG, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP,
    };

    /// The hook thread, told when watching starts or stops.
    static THREAD: AtomicU32 = AtomicU32::new(0);
    const WATCHING: u32 = WM_APP + 1;

    fn watching_changed(now: bool) {
        let thread = THREAD.load(Ordering::SeqCst);
        if thread != 0 {
            // SAFETY: posting a plain thread message carries no pointers.
            unsafe {
                let _ = PostThreadMessageW(thread, WATCHING, windows::Win32::Foundation::WPARAM(usize::from(now)), LPARAM(0));
            }
        }
    }

    fn program(window: HWND) -> Option<String> {
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(window, Some(&mut pid));
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let got = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
            let _ = CloseHandle(process);
            got.ok()?;
            Some(super::exe_name(&String::from_utf16_lossy(&buf[..len as usize])))
        }
    }

    fn top_level(window: HWND) -> bool {
        unsafe { GetAncestor(window, GA_ROOT) == window && IsWindowVisible(window).as_bool() }
    }

    unsafe extern "system" fn event(_: HWINEVENTHOOK, event: u32, window: HWND, object: i32, child: i32, _: u32, _: u32) {
        // Child windows and accessibility objects come through too: drop them first.
        if object != OBJID_WINDOW.0 || child != CHILDID_SELF as i32 {
            return;
        }
        match event {
            EVENT_OBJECT_DESTROY => {
                foxglove_plugin_host::apps::destroyed(window.0 as isize);
            }
            EVENT_OBJECT_SHOW if foxglove_plugin_host::apps::watching() && top_level(window) => {
                if let Some(exe) = program(window) {
                    foxglove_plugin_host::apps::shown(window.0 as isize, &exe);
                }
            }
            _ => {}
        }
    }

    /// The visible top-level windows open now, for a watch's first count.
    fn list() -> Vec<(isize, String)> {
        unsafe extern "system" fn each(window: HWND, found: LPARAM) -> BOOL {
            let found = unsafe { &mut *(found.0 as *mut Vec<(isize, String)>) };
            if top_level(window) {
                if let Some(exe) = program(window) {
                    found.push((window.0 as isize, exe));
                }
            }
            true.into()
        }
        let mut found: Vec<(isize, String)> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(each), LPARAM(&mut found as *mut _ as isize));
        }
        found
    }

    pub fn start() {
        foxglove_plugin_host::apps::set_lister(list);
        foxglove_plugin_host::apps::set_on_watching(watching_changed);
        // SAFETY: the hook is set and removed on this thread, whose message loop delivers its
        // events; posted messages carry only a flag.
        super::super::services::spawn_named("apps-watch", || unsafe {
            let mut msg = MSG::default();
            // Make the queue before anyone can post to it.
            let _ = windows::Win32::UI::WindowsAndMessaging::PeekMessageW(&mut msg, None, 0, 0, windows::Win32::UI::WindowsAndMessaging::PM_NOREMOVE);
            THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
            let mut hook: Option<HWINEVENTHOOK> = None;
            let set = |want: bool, hook: &mut Option<HWINEVENTHOOK>| match (want, hook.is_some()) {
                (true, false) => {
                    // DESTROY sits between CREATE and SHOW; CREATE is dropped (not shown yet).
                    let h = SetWinEventHook(EVENT_OBJECT_DESTROY, EVENT_OBJECT_SHOW, None, Some(event), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
                    if h.is_invalid() {
                        eprintln!("foxglove: apps watch: the window hook was refused");
                    } else {
                        *hook = Some(h);
                    }
                }
                (false, true) => {
                    if let Some(h) = hook.take() {
                        let _ = UnhookWinEvent(h);
                    }
                }
                _ => {}
            };
            // A watch that started before this thread was ready.
            set(foxglove_plugin_host::apps::watching(), &mut hook);
            // The hook's events arrive through this thread's messages.
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.hwnd.0.is_null() && msg.message == WATCHING {
                    // A start always turns it on; a stop only if no watch started since.
                    set(msg.wParam.0 != 0 || foxglove_plugin_host::apps::watching(), &mut hook);
                }
            }
        });
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn start() {}
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_path_becomes_its_file_name() {
        assert_eq!(super::exe_name(r"C:\Program Files\obs-studio\bin\64bit\OBS64.exe"), "obs64.exe");
        assert_eq!(super::exe_name("obs64.exe"), "obs64.exe");
    }
}
