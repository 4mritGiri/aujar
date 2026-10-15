//! Screen size from the kernel's DRM sysfs entries, for sessions that cannot be asked
//! (Wayland). It sees connected outputs and their preferred modes, but not their layout or
//! scaling, so outputs are assumed to sit side by side. `mouse_borders.screen` overrides it.

use std::path::Path;

use aujar_core::Size;

use crate::error::{Error, Result};

/// Parse a `modes` line such as `1920x1080` (the first line is the preferred mode).
pub fn parse_mode(line: &str) -> Option<Size> {
    let (width, height) = line.trim().split_once('x')?;
    // Interlaced modes carry a suffix: `1920x1080i`.
    let height = height.trim_end_matches(|c: char| !c.is_ascii_digit());

    Some(Size {
        width: width.parse().ok().filter(|w| *w > 0)?,
        height: height.parse().ok().filter(|h| *h > 0)?,
    })
}

/// Outputs side by side: widths add up, the tallest sets the height.
pub fn combine(sizes: &[Size]) -> Option<Size> {
    sizes.iter().copied().reduce(|total, next| Size {
        width: total.width.saturating_add(next.width),
        height: total.height.max(next.height),
    })
}

pub fn detect(root: &Path) -> Result<Size> {
    let mut sizes = Vec::new();

    for entry in std::fs::read_dir(root)?.flatten() {
        let path = entry.path();
        let connected = std::fs::read_to_string(path.join("status"))
            .map(|status| status.trim() == "connected")
            .unwrap_or(false);

        if !connected {
            continue;
        }

        if let Some(size) = std::fs::read_to_string(path.join("modes"))
            .ok()
            .and_then(|modes| modes.lines().next().and_then(parse_mode))
        {
            sizes.push(size);
        }
    }

    combine(&sizes).ok_or_else(|| Error::Other("no connected display found".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes() {
        assert_eq!(
            parse_mode("1920x1080\n"),
            Some(Size {
                width: 1920,
                height: 1080
            })
        );
        assert_eq!(
            parse_mode("1920x1080i"),
            Some(Size {
                width: 1920,
                height: 1080
            })
        );
        assert_eq!(parse_mode("garbage"), None);
        assert_eq!(parse_mode("0x1080"), None);
        assert_eq!(parse_mode(""), None);
    }

    #[test]
    fn outputs_sit_side_by_side() {
        let a = Size {
            width: 1920,
            height: 1080,
        };
        let b = Size {
            width: 2560,
            height: 1440,
        };

        assert_eq!(
            combine(&[a, b]),
            Some(Size {
                width: 4480,
                height: 1440
            })
        );
        assert_eq!(combine(&[]), None);
    }

    #[test]
    fn reads_connected_outputs_only() {
        let dir = tempfile::tempdir().unwrap();
        let make = |name: &str, status: &str, modes: &str| {
            let path = dir.path().join(name);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("status"), status).unwrap();
            std::fs::write(path.join("modes"), modes).unwrap();
        };

        make("card0-eDP-1", "connected\n", "1920x1080\n1280x720\n");
        make("card0-HDMI-A-1", "disconnected\n", "3840x2160\n");

        assert_eq!(
            detect(dir.path()).unwrap(),
            Size {
                width: 1920,
                height: 1080
            }
        );
        assert!(detect(&dir.path().join("card0-HDMI-A-1")).is_err());
    }
}
