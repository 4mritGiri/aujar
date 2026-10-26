//! macOS backends: a Quartz event tap to capture, posted Quartz events to inject, `arboard`
//! for the clipboard, `pmset`/`CGSession` to lock.
//!
//! **Not exercised on a real Mac yet**: it is type-checked against the macOS target and its
//! decision logic is unit-tested (`backend::macinput`), but nobody has run the event tap.
//! Treat it as experimental. The process needs *Input Monitoring* and *Accessibility*
//! permission (`aujar-mwb doctor` checks both).

mod capture;
mod desktop;
mod inject;
pub mod permissions;

use objc2_app_kit::NSPasteboard;

pub use capture::TapSource;
pub use desktop::{MacDesktop, MacLocker, virtual_bounds};
pub use inject::QuartzSink;

use super::{
    Backends, Clipboard, clipboard_arboard::ArboardClipboard, unavailable::UnavailableSource,
};
use crate::config::EngineConfig;

/// Password managers mark what they copy with this type so clipboard tools skip it.
const CONCEALED_TYPE: &str = "org.nspasteboard.ConcealedType";

fn clipboard_is_concealed() -> bool {
    let pasteboard = NSPasteboard::generalPasteboard();

    pasteboard
        .types()
        .is_some_and(|types| types.iter().any(|kind| kind.to_string() == CONCEALED_TYPE))
}

fn clipboard_token() -> Option<u64> {
    let count = NSPasteboard::generalPasteboard().changeCount();

    u64::try_from(count).ok()
}

pub fn backends(config: &EngineConfig) -> Backends {
    let mut notes = Vec::new();

    let source: Box<dyn super::InputSource> = match TapSource::new() {
        Ok(source) => Box::new(source),
        Err(error) => {
            // The first request shows macOS's own permission prompts.
            permissions::request_all();
            notes.push(error.to_string());

            Box::new(UnavailableSource::new(error.reason()))
        }
    };

    if !permissions::post_events_allowed() {
        notes.push(
            "this Mac cannot be controlled yet: allow this program under System Settings → \
             Privacy & Security → Accessibility"
                .to_owned(),
        );
    }

    let clipboard: Option<Box<dyn Clipboard>> = config.share_clipboard.then(|| {
        Box::new(ArboardClipboard::new(
            clipboard_is_concealed,
            clipboard_token,
        )) as Box<dyn Clipboard>
    });

    Backends {
        source,
        sink: Box::new(QuartzSink::new()),
        desktop: Box::new(MacDesktop::new()),
        clipboard,
        locker: Box::new(MacLocker),
        notes,
    }
}
