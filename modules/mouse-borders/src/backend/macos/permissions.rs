//! macOS privacy permissions that input capture and injection depend on.
//!
//! Capturing needs *Input Monitoring* (and Accessibility to swallow events); posting events
//! needs *Accessibility*. Both are granted per application in System Settings → Privacy &
//! Security, and macOS only asks the first time something requests access.

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
}

/// Whether this process may control the computer through the Accessibility API.
pub fn accessibility_trusted() -> bool {
    // SAFETY: a documented, argument-free query.
    unsafe { AXIsProcessTrusted() }
}

/// Whether this process may observe keyboard and mouse events (Input Monitoring).
pub fn input_monitoring_allowed() -> bool {
    // SAFETY: a documented, argument-free query.
    unsafe { CGPreflightListenEventAccess() }
}

/// Whether this process may post synthetic events.
pub fn post_events_allowed() -> bool {
    // SAFETY: a documented, argument-free query.
    unsafe { CGPreflightPostEventAccess() }
}

/// Ask macOS to show its permission prompts (does nothing for permissions already decided).
pub fn request_all() {
    // SAFETY: documented, argument-free requests.
    unsafe {
        CGRequestListenEventAccess();
        CGRequestPostEventAccess();
    }
}
