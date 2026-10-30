//! Icons embedded in the binary and served to GPUI's `svg()` element.
//! They are stroke-only 24x24 shapes; GPUI tints them with the element's text color.

use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

const ICONS: &[(&str, &[u8])] = &[
    (
        "icons/search.svg",
        include_bytes!("../assets/icons/search.svg"),
    ),
    (
        "icons/folder.svg",
        include_bytes!("../assets/icons/folder.svg"),
    ),
    (
        "icons/layers.svg",
        include_bytes!("../assets/icons/layers.svg"),
    ),
    ("icons/x.svg", include_bytes!("../assets/icons/x.svg")),
    (
        "icons/file-text.svg",
        include_bytes!("../assets/icons/file-text.svg"),
    ),
    (
        "icons/pdf.svg",
        include_bytes!("../assets/icons/pdf.svg"),
    ),
    (
        "icons/video.svg",
        include_bytes!("../assets/icons/video.svg"),
    ),
    (
        "icons/music.svg",
        include_bytes!("../assets/icons/music.svg"),
    ),
    (
        "icons/archive.svg",
        include_bytes!("../assets/icons/archive.svg"),
    ),
    (
        "icons/code.svg",
        include_bytes!("../assets/icons/code.svg"),
    ),
    (
        "icons/briefcase.svg",
        include_bytes!("../assets/icons/briefcase.svg"),
    ),
    (
        "icons/gamepad.svg",
        include_bytes!("../assets/icons/gamepad.svg"),
    ),
    (
        "icons/wrench.svg",
        include_bytes!("../assets/icons/wrench.svg"),
    ),
    ("icons/file.svg", include_bytes!("../assets/icons/file.svg")),
    ("icons/app.svg", include_bytes!("../assets/icons/app.svg")),
    (
        "icons/calculator.svg",
        include_bytes!("../assets/icons/calculator.svg"),
    ),
    (
        "icons/terminal.svg",
        include_bytes!("../assets/icons/terminal.svg"),
    ),
    (
        "icons/sliders.svg",
        include_bytes!("../assets/icons/sliders.svg"),
    ),
    (
        "icons/mouse.svg",
        include_bytes!("../assets/icons/mouse.svg"),
    ),
    (
        "icons/clipboard.svg",
        include_bytes!("../assets/icons/clipboard.svg"),
    ),
    (
        "icons/globe.svg",
        include_bytes!("../assets/icons/globe.svg"),
    ),
    ("icons/book.svg", include_bytes!("../assets/icons/book.svg")),
    ("icons/sun.svg", include_bytes!("../assets/icons/sun.svg")),
    ("icons/moon.svg", include_bytes!("../assets/icons/moon.svg")),
    (
        "icons/cloud.svg",
        include_bytes!("../assets/icons/cloud.svg"),
    ),
    ("icons/rain.svg", include_bytes!("../assets/icons/rain.svg")),
    ("icons/snow.svg", include_bytes!("../assets/icons/snow.svg")),
    (
        "icons/storm.svg",
        include_bytes!("../assets/icons/storm.svg"),
    ),
    ("icons/fog.svg", include_bytes!("../assets/icons/fog.svg")),
    (
        "icons/trend.svg",
        include_bytes!("../assets/icons/trend.svg"),
    ),
    (
        "icons/trophy.svg",
        include_bytes!("../assets/icons/trophy.svg"),
    ),
    (
        "icons/window.svg",
        include_bytes!("../assets/icons/window.svg"),
    ),
    (
        "icons/power.svg",
        include_bytes!("../assets/icons/power.svg"),
    ),
    (
        "icons/clock.svg",
        include_bytes!("../assets/icons/clock.svg"),
    ),
    (
        "icons/arrow-up-right.svg",
        include_bytes!("../assets/icons/arrow-up-right.svg"),
    ),
    (
        "icons/enter.svg",
        include_bytes!("../assets/icons/enter.svg"),
    ),
    (
        "icons/alert.svg",
        include_bytes!("../assets/icons/alert.svg"),
    ),
    (
        "icons/loader.svg",
        include_bytes!("../assets/icons/loader.svg"),
    ),
    (
        "icons/sidebar.svg",
        include_bytes!("../assets/icons/sidebar.svg"),
    ),
    (
        "icons/chevron-left.svg",
        include_bytes!("../assets/icons/chevron-left.svg"),
    ),
    (
        "icons/chevron-right.svg",
        include_bytes!("../assets/icons/chevron-right.svg"),
    ),
    (
        "icons/chevron-down.svg",
        include_bytes!("../assets/icons/chevron-down.svg"),
    ),
    ("icons/more.svg", include_bytes!("../assets/icons/more.svg")),
    (
        "icons/database.svg",
        include_bytes!("../assets/icons/database.svg"),
    ),
    (
        "icons/history.svg",
        include_bytes!("../assets/icons/history.svg"),
    ),
    ("icons/info.svg", include_bytes!("../assets/icons/info.svg")),
    (
        "icons/keyboard.svg",
        include_bytes!("../assets/icons/keyboard.svg"),
    ),
    ("icons/move.svg", include_bytes!("../assets/icons/move.svg")),
    (
        "icons/image.svg",
        include_bytes!("../assets/icons/image.svg"),
    ),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}
