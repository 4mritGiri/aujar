//! Picking a color from the screen and keeping the history.
//!
//! The picking itself is the desktop's own color picker (the daemon asks the
//! `org.freedesktop.portal.Screenshot.PickColor` portal), so Aujar never takes or keeps a
//! picture of the screen. A picked color is copied in the default format, added to the history
//! and shown in the editor.

use crate::{UI_SENDER, UiMessage};
use aujar_color::Rgb;
use aujar_config::Settings;
use gpui::App;
use std::{fs, path::PathBuf, time::Duration};

/// How many colors the history keeps.
pub(crate) const HISTORY_LIMIT: usize = 20;

/// How long to wait for the user to pick (or cancel) in the desktop's picker.
const PICK_TIMEOUT: Duration = Duration::from_secs(125);

// ---- Color history and copying ---------------------------------------------------------

pub(crate) fn history_path() -> PathBuf {
    Settings::path().with_file_name("color-history.txt")
}

/// The picked colors, newest first.
pub(crate) fn load_history() -> Vec<Rgb> {
    fs::read_to_string(history_path())
        .unwrap_or_default()
        .lines()
        .filter_map(Rgb::parse_hex)
        .fold(Vec::new(), |mut colors, color| {
            if !colors.contains(&color) && colors.len() < HISTORY_LIMIT {
                colors.push(color);
            }

            colors
        })
}

pub(crate) fn save_history(colors: &[Rgb]) {
    let path = history_path();

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let text = colors
        .iter()
        .map(|color| color.to_hex())
        .collect::<Vec<_>>()
        .join("\n");

    let _ = fs::write(path, text);
}

/// Put a color at the top of the history and return the new history.
pub(crate) fn remember(color: Rgb) -> Vec<Rgb> {
    let mut colors = load_history();

    colors.retain(|existing| *existing != color);
    colors.insert(0, color);
    colors.truncate(HISTORY_LIMIT);
    save_history(&colors);

    colors
}

/// The color in the format picked as the default in settings (HEX when there is none).
pub(crate) fn default_text(settings: &Settings, color: Rgb) -> String {
    default_text_with_alpha(settings, color, 255)
}

/// Like [`default_text`] for a color that is not fully opaque (`alpha` is `0..=255`): formats
/// with `%Al` carry it, and the HEX fallback gets two more digits.
pub(crate) fn default_text_with_alpha(settings: &Settings, color: Rgb, alpha: u8) -> String {
    let picker = &settings.color_picker;

    picker
        .formats
        .iter()
        .find(|format| format.enabled && format.name.eq_ignore_ascii_case(&picker.default_format))
        .map_or_else(
            || {
                if alpha == 255 {
                    color.to_hex()
                } else {
                    format!("{}{alpha:02X}", color.to_hex())
                }
            },
            |format| color.format_with_alpha(&format.pattern, alpha),
        )
}

/// Tell the UI thread something (the windows talk to it through messages).
pub(crate) fn send_to_ui(message: UiMessage) {
    if let Some(sender) = UI_SENDER.get() {
        let _ = sender.try_send(message);
    }
}

// ---- Picking ------------------------------------------------------------------------------

/// Let the user pick a pixel with the desktop's own color picker. The daemon makes the portal
/// call (and checks the `screen_capture` policy); the answer comes back as `ColorPicked`.
pub(crate) fn start_pick(cx: &mut App) {
    if !Settings::load_or_default().color_picker.enabled {
        return;
    }

    cx.spawn(async move |cx| {
        let answer = cx
            .background_executor()
            .spawn(async {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())?;

                runtime.block_on(async {
                    tokio::time::timeout(
                        PICK_TIMEOUT,
                        aujar_ipc::send(crate::socket_path(), aujar_ipc::Request::PickColor),
                    )
                    .await
                    .map_err(|_| "timed out waiting for a color".to_owned())?
                    .map_err(|error| error.to_string())
                })
            })
            .await;

        match answer {
            Ok(aujar_ipc::Response::Color(color)) => {
                if let Some(color) = Rgb::parse_hex(&color.hex) {
                    send_to_ui(UiMessage::ColorPicked(color));
                }
            }
            Ok(aujar_ipc::Response::Error(message)) | Err(message) => {
                crate::debug(&format!("color picker: nothing picked ({message})"));
            }
            Ok(_) => {}
        }
    })
    .detach();
}

/// A color was picked: copy it in the default format, remember it, show it in the editor.
pub(crate) fn picked(cx: &mut App, state: &mut crate::UiState, color: Rgb) {
    let settings = Settings::load_or_default();

    cx.write_to_clipboard(gpui::ClipboardItem::new_string(default_text(
        &settings, color,
    )));
    remember(color);
    open_editor(cx, state);
}

/// Show the editor, replacing one that is open: it reads the history again and comes to front.
pub(crate) fn open_editor(cx: &mut App, state: &mut crate::UiState) {
    if let Some(handle) = state.color_editor.take() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }

    state.color_editor = crate::color_editor::open_window(cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_format_renders_without_leftover_tokens() {
        let settings = Settings::default();

        for color in [
            Rgb::new(0, 0, 0),
            Rgb::new(255, 255, 255),
            Rgb::new(255, 136, 0),
            Rgb::new(108, 140, 255),
        ] {
            for format in &settings.color_picker.formats {
                let text = color.format(&format.pattern);

                // A `%` may stay (`50%`), but never a token name such as `%Re` or `%Hu`.
                assert!(
                    ![
                        "%Re", "%Gr", "%Bl", "%Hu", "%Sl", "%Ll", "%Sb", "%Va", "%Br", "%Cy",
                        "%Ma", "%Ye", "%Bk", "%Si", "%In", "%Hn", "%Wh", "%Bn", "%Xv", "%Yv",
                        "%Zv", "%Lc", "%Ca", "%Cb", "%Dv", "%Lo", "%Co", "%Ho", "%Al"
                    ]
                    .iter()
                    .any(|token| text.contains(token)),
                    "{} left a token in {text}",
                    format.name
                );
                assert!(!text.is_empty(), "{} is empty", format.name);
            }
        }
    }

    #[test]
    fn the_default_format_is_hex_without_a_hash() {
        let settings = Settings::default();

        assert_eq!(default_text(&settings, Rgb::new(255, 136, 0)), "FF8800");
    }

    #[test]
    fn an_older_hex_format_still_copies_with_the_hash() {
        let mut settings = Settings::default();

        settings.color_picker.formats = vec![aujar_config::ColorFormat {
            name: "HEX".into(),
            pattern: "{hex}".into(),
            enabled: true,
        }];

        assert_eq!(default_text(&settings, Rgb::new(255, 136, 0)), "#FF8800");
    }

    #[test]
    fn alpha_reaches_the_formats_that_ask_for_it() {
        let mut settings = Settings::default();

        settings.color_picker.formats = vec![aujar_config::ColorFormat {
            name: "HEX".into(),
            pattern: "%ReX%GrX%BlX%AlX".into(),
            enabled: true,
        }];

        assert_eq!(
            default_text_with_alpha(&settings, Rgb::new(255, 136, 0), 128),
            "FF880080"
        );
        assert_eq!(default_text(&settings, Rgb::new(255, 136, 0)), "FF8800FF");

        // With no matching format the HEX fallback carries the alpha.
        settings.color_picker.default_format = "Nope".into();

        assert_eq!(
            default_text_with_alpha(&settings, Rgb::new(255, 136, 0), 128),
            "#FF880080"
        );
    }

    #[test]
    fn an_unknown_default_format_falls_back_to_hex() {
        let mut settings = Settings::default();

        settings.color_picker.default_format = "Nope".into();

        assert_eq!(default_text(&settings, Rgb::new(255, 136, 0)), "#FF8800");
    }
}
