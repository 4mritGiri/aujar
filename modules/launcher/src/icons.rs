//! Minimal freedesktop icon lookup: find a PNG (preferred) or SVG file for an icon name.
//!
//! This is deliberately simpler than the full icon-theme spec: it probes the common
//! themes and sizes, which covers GNOME, KDE, Flatpak and most packaged apps.

use std::path::{Path, PathBuf};

const THEMES: [&str; 6] = ["hicolor", "Yaru", "Adwaita", "Papirus", "breeze", "gnome"];
const PNG_SIZES: [&str; 6] = ["256x256", "128x128", "96x96", "64x64", "48x48", "512x512"];

fn base_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/share/icons"));
        dirs.push(home.join(".icons"));
        dirs.push(home.join(".local/share/flatpak/exports/share/icons"));
    }

    dirs.push(PathBuf::from("/usr/share/icons"));
    dirs.push(PathBuf::from("/usr/local/share/icons"));
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/icons"));
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/icons"));

    dirs
}

fn is_supported_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "svg")
    )
}

/// Resolve an `Icon=` value (theme name or absolute path) to an existing image file.
pub fn resolve_icon(name: &str) -> Option<String> {
    let name = name.trim();

    if name.is_empty() || name.contains("..") {
        return None;
    }

    let path = Path::new(name);

    if path.is_absolute() {
        return (path.is_file() && is_supported_image(path)).then(|| name.to_owned());
    }

    let bases = base_dirs();

    // Bitmaps first: they render everywhere.
    for size in PNG_SIZES {
        for base in &bases {
            for theme in THEMES {
                let candidate = base
                    .join(theme)
                    .join(size)
                    .join("apps")
                    .join(format!("{name}.png"));

                if candidate.is_file() {
                    return Some(candidate.to_string_lossy().into_owned());
                }
            }
        }
    }

    for base in &bases {
        for theme in THEMES {
            let candidate = base
                .join(theme)
                .join("scalable/apps")
                .join(format!("{name}.svg"));

            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }

    for extension in ["png", "svg"] {
        let candidate = PathBuf::from(format!("/usr/share/pixmaps/{name}.{extension}"));

        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_empty_names() {
        assert_eq!(resolve_icon(""), None);
        assert_eq!(resolve_icon("../../etc/passwd"), None);
    }

    #[test]
    fn missing_absolute_path_is_none() {
        assert_eq!(resolve_icon("/definitely/not/here.png"), None);
    }

    #[test]
    fn only_png_and_svg_are_supported() {
        assert!(is_supported_image(Path::new("/a/b.PNG")));
        assert!(is_supported_image(Path::new("/a/b.svg")));
        assert!(!is_supported_image(Path::new("/a/b.xpm")));
    }
}
