//! A picture of the whole screen, for the color picker overlay.
//!
//! Wayland does not let an app read other windows' pixels, so the picker asks the desktop for a
//! screenshot (`org.freedesktop.portal.Screenshot`), shows it frozen full screen and reads the
//! colors from it. The screenshot file is deleted as soon as it is loaded.

use crate::Rgb;
#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::Result;
#[cfg(target_os = "linux")]
use std::time::Duration;

/// How long to wait for the desktop to hand over the screenshot (it may ask permission first).
#[cfg(target_os = "linux")]
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(60);

/// The screen as RGBA pixels, row by row, in physical pixels.
pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Capture {
    /// The color at a pixel, or `None` outside the picture.
    pub fn pixel(&self, x: i64, y: i64) -> Option<Rgb> {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return None;
        }

        let at = (y as usize * self.width as usize + x as usize) * 4;
        let pixel = self.rgba.get(at..at + 3)?;

        Some(Rgb::new(pixel[0], pixel[1], pixel[2]))
    }
}

/// Take a screenshot of the whole desktop (every monitor, side by side).
///
/// The portal call runs on a blocking thread with its own runtime, like [`crate::pick`].
pub async fn capture_screen() -> Result<Capture> {
    #[cfg(not(target_os = "linux"))]
    anyhow::bail!("capturing the screen is not implemented for this platform yet");

    #[cfg(target_os = "linux")]
    {
        tokio::task::spawn_blocking(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to start a runtime for the screen capture")?;

            runtime.block_on(async {
                tokio::time::timeout(CAPTURE_TIMEOUT, capture_inner())
                    .await
                    .context("timed out waiting for the desktop to take a screenshot")?
            })
        })
        .await
        .context("the screen capture task failed")?
    }
}

#[cfg(target_os = "linux")]
async fn capture_inner() -> Result<Capture> {
    use ashpd::desktop::screenshot::Screenshot;

    let shot = Screenshot::request()
        .interactive(false)
        .send()
        .await
        .context("the screenshot portal is not available on this desktop")?
        .response()
        .context("the screenshot was cancelled or not allowed")?;

    let path = file_uri_to_path(shot.uri().as_str())
        .with_context(|| format!("unexpected screenshot location: {}", shot.uri().as_str()))?;

    let decoded = image::open(&path);

    // A picture of everything on screen must not stay on disk.
    let _ = std::fs::remove_file(&path);

    let picture = decoded
        .with_context(|| format!("could not read the screenshot {}", path.display()))?
        .to_rgba8();

    Ok(Capture {
        width: picture.width(),
        height: picture.height(),
        rgba: picture.into_raw(),
    })
}

/// `file:///home/me/Pictures/Screenshot%20from%20now.png` to a path.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn file_uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // Skip an optional host (`file://localhost/...`).
    let path = &rest[rest.find('/')?..];
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;

            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    Some(std::path::PathBuf::from(String::from_utf8(decoded).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pixels_inside_the_picture_only() {
        let capture = Capture {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 128, 255, 255],
        };

        assert_eq!(capture.pixel(0, 0), Some(Rgb::new(255, 0, 0)));
        assert_eq!(capture.pixel(1, 0), Some(Rgb::new(0, 128, 255)));
        assert_eq!(capture.pixel(2, 0), None);
        assert_eq!(capture.pixel(0, 1), None);
        assert_eq!(capture.pixel(-1, 0), None);
    }

    #[test]
    fn decodes_file_uris() {
        assert_eq!(
            file_uri_to_path("file:///home/me/Pictures/Screenshot%20from%20now.png"),
            Some("/home/me/Pictures/Screenshot from now.png".into())
        );
        assert_eq!(
            file_uri_to_path("file://localhost/tmp/a.png"),
            Some("/tmp/a.png".into())
        );
        assert_eq!(file_uri_to_path("https://example.com/a.png"), None);
        assert_eq!(file_uri_to_path("file:///bad%zz"), None);
    }
}
