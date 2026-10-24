//! Blocking Windows watches live away from the UI and panel worker.
#[cfg(windows)]
mod imp {
    use std::sync::Arc;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED_0, WAIT_OBJECT_0};
    use windows::Win32::Storage::FileSystem::*;
    use windows::Win32::System::Threading::*;

    pub struct Watch {
        cancel: HANDLE,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for Watch {
        fn drop(&mut self) {
            unsafe {
                let _ = SetEvent(self.cancel);
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            unsafe {
                let _ = CloseHandle(self.cancel);
            }
        }
    }
    fn watch(handle: HANDLE, directory: bool, wake: Arc<dyn Fn() + Send + Sync>) -> Option<Watch> {
        let cancel = match unsafe { CreateEventW(None, true, false, None) } {
            Ok(h) => h,
            Err(_) => {
                unsafe {
                    if directory {
                        let _ = FindCloseChangeNotification(handle);
                    } else {
                        let _ = CloseHandle(handle);
                    }
                }
                return None;
            }
        };
        let raw = (handle.0 as usize, cancel.0 as usize);
        let thread = std::thread::Builder::new()
            .name("panel-watch".into())
            .spawn(move || unsafe {
                let handle = HANDLE(raw.0 as *mut _);
                let cancel = HANDLE(raw.1 as *mut _);
                loop {
                    let result = WaitForMultipleObjects(&[cancel, handle], false, INFINITE);
                    if result.0 != WAIT_OBJECT_0.0 + 1 && result.0 != WAIT_ABANDONED_0.0 + 1 {
                        break;
                    }
                    if directory {
                        // Rearm before enqueueing, so another write cannot fall in the gap.
                        let ok = FindNextChangeNotification(handle).is_ok();
                        wake();
                        if !ok {
                            break;
                        }
                    } else {
                        let _ = ReleaseMutex(handle);
                        wake();
                        break;
                    }
                }
                if directory {
                    let _ = FindCloseChangeNotification(handle);
                } else {
                    let _ = CloseHandle(handle);
                }
            });
        match thread {
            Ok(thread) => Some(Watch {
                cancel,
                thread: Some(thread),
            }),
            Err(_) => {
                unsafe {
                    let _ = CloseHandle(cancel);
                    if directory {
                        let _ = FindCloseChangeNotification(handle);
                    } else {
                        let _ = CloseHandle(handle);
                    }
                }
                None
            }
        }
    }
    pub fn settings(path: &std::path::Path, wake: Arc<dyn Fn() + Send + Sync>) -> Option<Watch> {
        let dir = path.parent()?;
        std::fs::create_dir_all(dir).ok()?;
        let h = unsafe {
            FindFirstChangeNotificationW(
                &HSTRING::from(dir),
                false,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_SIZE,
            )
        }
        .ok()?;
        watch(h, true, wake)
    }
    pub fn panel(wake: Arc<dyn Fn() + Send + Sync>) -> Option<Watch> {
        let h = unsafe {
            CreateMutexW(
                None,
                false,
                &HSTRING::from(foxglove_panel::panel_lock::NAME),
            )
        }
        .ok()?;
        watch(h, false, wake)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::mpsc;
        use std::time::{Duration, SystemTime, UNIX_EPOCH};

        fn unique() -> String {
            format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )
        }

        #[test]
        fn directory_watch_reports_creation_and_can_be_cancelled_while_idle() {
            let dir = std::env::temp_dir().join(format!("foxglove-watch-{}", unique()));
            let path = dir.join("screens.ini");
            let (tx, rx) = mpsc::channel();
            let watcher = settings(
                &path,
                Arc::new(move || {
                    let _ = tx.send(());
                }),
            )
            .unwrap();
            std::fs::write(&path, "[screens]\ndrive_left=false\n").unwrap();
            rx.recv_timeout(Duration::from_secs(2)).unwrap();
            drop(watcher);
            std::fs::remove_file(path).unwrap();
            std::fs::remove_dir(dir).unwrap();
        }

        #[test]
        fn mutex_watch_wakes_on_release_without_taking_ownership_from_the_worker() {
            let name = HSTRING::from(format!(r"Local\Foxglove.WatchTest.{}", unique()));
            let holder = unsafe { CreateMutexW(None, true, &name) }.unwrap();
            let handle = unsafe { CreateMutexW(None, false, &name) }.unwrap();
            let (tx, rx) = mpsc::channel();
            let watcher = watch(
                handle,
                false,
                Arc::new(move || {
                    let _ = tx.send(());
                }),
            )
            .unwrap();
            assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
            unsafe { ReleaseMutex(holder) }.unwrap();
            rx.recv_timeout(Duration::from_secs(2)).unwrap();
            drop(watcher);
            assert_eq!(unsafe { WaitForSingleObject(holder, 0) }, WAIT_OBJECT_0);
            unsafe { ReleaseMutex(holder) }.unwrap();
            unsafe { CloseHandle(holder) }.unwrap();
        }
    }
}
#[cfg(not(windows))]
mod imp {
    pub struct Watch;
    pub fn settings(
        _: &std::path::Path,
        _: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Option<Watch> {
        None
    }
    pub fn panel(_: std::sync::Arc<dyn Fn() + Send + Sync>) -> Option<Watch> {
        None
    }
}
pub use imp::*;
