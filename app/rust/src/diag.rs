//! Temporary on-device diagnostics for the Android remote-PDF investigation.
//!
//! Keep this deliberately tiny and dependency-free so release builds can emit
//! stage markers directly to logcat even when no tracing subscriber is installed.

#[cfg(target_os = "android")]
use std::ffi::CString;
#[cfg(target_os = "android")]
use std::os::raw::{c_char, c_int};

#[cfg(target_os = "android")]
#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

/// Emit one compact diagnostic marker.
///
/// Android: native logcat tag `RCH-PDF-DIAG`.
/// Other targets: stderr, which keeps unit tests observable without extra deps.
pub(crate) fn pdf_diag(message: impl AsRef<str>) {
    let message = message.as_ref();

    #[cfg(target_os = "android")]
    {
        const ANDROID_LOG_INFO: c_int = 4;
        let Ok(tag) = CString::new("RCH-PDF-DIAG") else {
            return;
        };
        let sanitized = message.replace('\0', "?");
        let Ok(text) = CString::new(sanitized) else {
            return;
        };
        unsafe {
            let _ = __android_log_write(ANDROID_LOG_INFO, tag.as_ptr(), text.as_ptr());
        }
    }

    #[cfg(not(target_os = "android"))]
    eprintln!("[RCH-PDF-DIAG] {message}");
}
