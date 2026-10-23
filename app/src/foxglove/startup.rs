//! Report native window creation failures without Rust's termination error output.

pub fn finish(result: Result<(), Box<dyn std::error::Error>>) {
    if let Err(error) = result {
        let message = failure_message(&error.to_string(), cfg!(target_os = "windows"));
        eprintln!("{message}");
        #[cfg(target_os = "windows")]
        show_message(&message);
        std::process::exit(1);
    }
}

fn failure_message(error: &str, windows: bool) -> String {
    let mut message = format!("Foxglove could not open its window.\n\n{error}");
    if windows {
        // Window creation can also fail for other reasons; preserve the original error.
        message.push_str("\n\nThe Windows global atom table may be full because another program is leaking atoms. If it is full, signing out of Windows or rebooting clears it. Close or update the leaking program to prevent this happening again.");
    }
    message
}

#[cfg(target_os = "windows")]
fn show_message(message: &str) {
    use windows::core::{w, PCWSTR};
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    // Only reached after the event loop has failed: no live UI or link waits on this dialog.
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("Foxglove could not start"),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_failure_preserves_error_and_explains_recovery() {
        let message = failure_message("OS error 8 (0x80070008)", true);
        assert!(message.contains("OS error 8 (0x80070008)"));
        assert!(message.contains("atom table"));
        assert!(message.contains("signing out"));
        assert!(message.contains("rebooting"));
    }

    #[test]
    fn other_platforms_only_report_the_original_failure() {
        assert_eq!(
            failure_message("display unavailable", false),
            "Foxglove could not open its window.\n\ndisplay unavailable"
        );
    }
}
