//! The vendored `accesskit_windows` patch (BanditF/foxglove#1381): a failed subclass install
//! (Windows' global atom table full) is an error, not a panic. No window is created: the
//! handle below is not a window, so `SetPropW` fails the same way it does with a full atom
//! table, and nothing is shown on the desktop.
#![cfg(windows)]

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, TreeUpdate};
use accesskit_windows::SubclassingAdapter;
use windows_061::Win32::Foundation::HWND;

struct Quiet;

impl ActivationHandler for Quiet {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        None
    }
}

impl ActionHandler for Quiet {
    fn do_action(&mut self, _: ActionRequest) {}
}

fn not_a_window() -> HWND {
    HWND(0x7FFF_FFF0 as *mut core::ffi::c_void)
}

#[test]
fn a_failed_install_is_an_error_not_a_panic() {
    match SubclassingAdapter::try_new(not_a_window(), Quiet, Quiet) {
        Ok(_) => panic!("a handle that is not a window cannot be subclassed"),
        Err((mut inert, _err)) => {
            // The inert adapter never has a tree, and dropping it is quiet.
            assert!(inert
                .update_if_active(|| -> TreeUpdate { unreachable!("inert") })
                .is_none());
        }
    }
}

#[test]
fn the_infallible_constructor_degrades_the_same_way() {
    let mut inert = SubclassingAdapter::new(not_a_window(), Quiet, Quiet);
    assert!(inert
        .update_if_active(|| -> TreeUpdate { unreachable!("inert") })
        .is_none());
}
