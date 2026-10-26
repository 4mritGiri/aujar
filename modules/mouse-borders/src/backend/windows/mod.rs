//! Windows backends: low-level hooks to capture, `SendInput` to inject, `arboard` for the
//! clipboard, `LockWorkStation` to lock.
//!
//! **Not exercised on a real Windows machine yet**: it is type-checked against the Windows
//! target and its decision logic is unit-tested (`backend::winput`), but nobody has run the
//! hooks. Treat it as experimental.

mod capture;
mod desktop;
mod inject;

use windows_sys::Win32::{
    System::DataExchange::{
        GetClipboardSequenceNumber, IsClipboardFormatAvailable, RegisterClipboardFormatW,
    },
    UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
};

pub use capture::HookSource;
pub use desktop::{VirtualScreen, WindowsDesktop, WindowsLocker, virtual_screen};
pub use inject::SendInputSink;

use super::{
    Backends, Clipboard, clipboard_arboard::ArboardClipboard, unavailable::UnavailableSource,
};
use crate::config::EngineConfig;

/// Call once at startup. Without per-monitor DPI awareness Windows reports scaled coordinates
/// and screen sizes, which would make machines disagree about where the screen edges are.
pub fn init_process() {
    // SAFETY: plain Win32 call; failure (already set by a manifest) is fine.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Password managers mark what they copy with this format so history tools skip it.
fn clipboard_is_concealed() -> bool {
    let name: Vec<u16> = "ExcludeClipboardContentFromMonitorProcessing\0"
        .encode_utf16()
        .collect();

    // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the calls.
    unsafe {
        let format = RegisterClipboardFormatW(name.as_ptr());

        format != 0 && IsClipboardFormatAvailable(format) != 0
    }
}

fn clipboard_token() -> Option<u64> {
    // SAFETY: plain Win32 call.
    Some(u64::from(unsafe { GetClipboardSequenceNumber() }))
}

pub fn backends(config: &EngineConfig) -> Backends {
    let mut notes = Vec::new();

    let source: Box<dyn super::InputSource> = match HookSource::new() {
        Ok(source) => Box::new(source),
        Err(error) => {
            notes.push(error.to_string());

            Box::new(UnavailableSource::new(error.reason()))
        }
    };

    let clipboard: Option<Box<dyn Clipboard>> = config.share_clipboard.then(|| {
        Box::new(ArboardClipboard::new(
            clipboard_is_concealed,
            clipboard_token,
        )) as Box<dyn Clipboard>
    });

    Backends {
        source,
        sink: Box::new(SendInputSink),
        desktop: Box::new(WindowsDesktop),
        clipboard,
        locker: Box::new(WindowsLocker),
        notes,
    }
}
