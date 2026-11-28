//! The screen, on every platform: a picture of it, where the monitors are in that picture, and
//! where the windows are. Aujar Snip and Text Extractor build on this.
//!
//! | | Linux | Windows | macOS |
//! |---|---|---|---|
//! | Picture | screenshot portal (GNOME, KDE), `grim` (wlroots), `maim`/`scrot` (X11) | GDI copy of the virtual screen, per-monitor DPI aware | CoreGraphics, one image per display |
//! | Monitors | not known (the UI uses its own display list) | `EnumDisplayMonitors` + DPI | `CGGetActiveDisplayList` |
//! | Windows | Aujar Shell extension (GNOME), `swaymsg`, `hyprctl` | `EnumWindows` + DWM frame bounds | `CGWindowListCopyWindowInfo` |
//! | Permission | GNOME: once, `aujar snip allow` | none | Screen Recording, once |

pub mod capture;
pub mod permission;
pub mod windows;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod win;

pub use capture::{Backend, Cancelled};

/// One monitor: where it is on the desktop (logical pixels, the layout the desktop shows) and
/// where its pixels are in a captured picture (physical pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    /// Logical bounds on the desktop.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Its rectangle in the picture.
    pub picture_x: u32,
    pub picture_y: u32,
    pub picture_width: u32,
    pub picture_height: u32,
}

/// A picture of every monitor and where each one is in it.
pub struct Screen {
    pub picture: image::RgbaImage,
    /// Empty when the platform does not say (the Linux portal and tools).
    pub monitors: Vec<Monitor>,
}
