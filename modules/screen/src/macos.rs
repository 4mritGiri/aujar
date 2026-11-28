//! macOS: one CoreGraphics image per display, and windows from the window server.
//!
//! Without the Screen Recording permission macOS still returns pictures, but of the desktop
//! wallpaper only, and windows without titles. So the permission is checked first and, the first
//! time, requested (System Settings > Privacy & Security > Screen Recording).
//!
//! `CGDisplayCreateImage` is deprecated in favour of ScreenCaptureKit; it is used because it is
//! synchronous and works on every supported macOS. Moving to ScreenCaptureKit is a known next step.

use crate::{Monitor, Screen, windows::WindowRect};
use anyhow::{Result, bail};
use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use core_graphics::{
    display::CGDisplay,
    window::{
        copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName,
        kCGWindowOwnerName,
    },
};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Whether Aujar may record the screen. Asks once (the system shows its prompt) when `ask`.
pub(crate) fn screen_recording_allowed(ask: bool) -> bool {
    // SAFETY: plain calls without arguments.
    unsafe { CGPreflightScreenCaptureAccess() || (ask && CGRequestScreenCaptureAccess()) }
}

pub(crate) fn capture() -> Result<Screen> {
    if !screen_recording_allowed(true) {
        bail!(
            "macOS has not allowed Aujar to record the screen. Turn it on in System Settings > \
             Privacy & Security > Screen Recording, then restart Aujar"
        );
    }

    let ids = CGDisplay::active_displays()
        .map_err(|error| anyhow::anyhow!("macOS could not list the displays (error {error})"))?;

    let mut shots = Vec::new();

    for id in ids {
        let display = CGDisplay::new(id);
        let Some(image) = display.image() else {
            continue;
        };

        if image.bits_per_pixel() != 32 {
            bail!(
                "unexpected display pixel format ({} bits)",
                image.bits_per_pixel()
            );
        }

        let (width, height, stride) = (image.width(), image.height(), image.bytes_per_row());
        let data = image.data();
        let bytes = data.bytes();
        let mut rgba = Vec::with_capacity(width * height * 4);

        // 32-bit little-endian, alpha first: B, G, R, A in memory.
        for row in 0..height {
            let line = &bytes[row * stride..row * stride + width * 4];

            for pixel in line.chunks_exact(4) {
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
        }

        let bounds = display.bounds();

        shots.push((
            image::RgbaImage::from_raw(width as u32, height as u32, rgba)
                .ok_or_else(|| anyhow::anyhow!("the display picture has the wrong size"))?,
            bounds,
        ));
    }

    if shots.is_empty() {
        bail!("macOS returned no display pictures");
    }

    // Side by side in one picture; each monitor says where its part is.
    let total_width: u32 = shots.iter().map(|(picture, _)| picture.width()).sum();
    let max_height = shots
        .iter()
        .map(|(picture, _)| picture.height())
        .max()
        .unwrap_or(1);
    let mut picture = image::RgbaImage::new(total_width, max_height);
    let mut monitors = Vec::new();
    let mut x = 0u32;

    for (shot, bounds) in &shots {
        image::imageops::replace(&mut picture, shot, i64::from(x), 0);

        monitors.push(Monitor {
            x: bounds.origin.x as f32,
            y: bounds.origin.y as f32,
            width: bounds.size.width as f32,
            height: bounds.size.height as f32,
            picture_x: x,
            picture_y: 0,
            picture_width: shot.width(),
            picture_height: shot.height(),
        });

        x += shot.width();
    }

    Ok(Screen { picture, monitors })
}

fn number(dictionary: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<f64> {
    dictionary
        .find(key)
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|number| number.to_f64())
}

fn string(dictionary: &CFDictionary<CFString, CFType>, key: &CFString) -> String {
    dictionary
        .find(key)
        .and_then(|value| value.downcast::<CFString>())
        .map(|text| text.to_string())
        .unwrap_or_default()
}

/// On-screen application windows (layer 0), front to back, in points (top-left origin, the
/// same space as the display bounds).
pub(crate) fn visible_windows() -> Vec<WindowRect> {
    let Some(list) = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    ) else {
        return Vec::new();
    };

    // SAFETY: the window server's keys are valid static CFStrings.
    let (bounds_key, layer_key, owner_key, name_key) = unsafe {
        (
            CFString::wrap_under_get_rule(kCGWindowBounds),
            CFString::wrap_under_get_rule(kCGWindowLayer),
            CFString::wrap_under_get_rule(kCGWindowOwnerName),
            CFString::wrap_under_get_rule(kCGWindowName),
        )
    };
    let (kx, ky, kw, kh) = (
        CFString::from_static_string("X"),
        CFString::from_static_string("Y"),
        CFString::from_static_string("Width"),
        CFString::from_static_string("Height"),
    );

    let list: CFArray<CFDictionary<CFString, CFType>> =
        // SAFETY: the window list is an array of dictionaries keyed by CFString.
        unsafe { CFArray::wrap_under_get_rule(list.as_concrete_TypeRef()) };
    let mut out = Vec::new();

    for (index, window) in list.iter().enumerate() {
        // Menus, the Dock, overlays and the like sit on other layers.
        if number(&window, &layer_key) != Some(0.0) {
            continue;
        }

        let Some(bounds) = window
            .find(&bounds_key)
            .and_then(|value| value.downcast::<CFDictionary>())
        else {
            continue;
        };
        // SAFETY: the bounds dictionary is keyed by CFString with CFNumber values.
        let bounds: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bounds.as_concrete_TypeRef()) };

        let (Some(x), Some(y), Some(width), Some(height)) = (
            number(&bounds, &kx),
            number(&bounds, &ky),
            number(&bounds, &kw),
            number(&bounds, &kh),
        ) else {
            continue;
        };

        out.push(WindowRect {
            title: string(&window, &name_key),
            app: string(&window, &owner_key),
            x: x.round() as i32,
            y: y.round() as i32,
            width: width.round() as i32,
            height: height.round() as i32,
            // The list is front to back.
            stack: -(index as i32),
        });
    }

    out
}
