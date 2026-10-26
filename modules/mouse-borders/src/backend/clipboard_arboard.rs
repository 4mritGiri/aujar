//! Clipboard access through `arboard`, for Windows and macOS (where there are no
//! `wl-clipboard`/`xclip`-style tools to lean on).
//!
//! Each operation opens the clipboard afresh on a blocking thread: `arboard`'s handle is not
//! meant to be shared across threads. Content the source application marks as secret (a
//! password manager) is never read.

use std::borrow::Cow;

use async_trait::async_trait;

use super::{Clipboard, ClipboardContent};
use crate::{
    error::{Error, Result},
    image_codec,
};

pub struct ArboardClipboard {
    /// Whether the clipboard currently holds content flagged as secret.
    concealed: fn() -> bool,
    /// The platform's cheap "clipboard changed" counter.
    token: fn() -> Option<u64>,
}

impl ArboardClipboard {
    pub fn new(concealed: fn() -> bool, token: fn() -> Option<u64>) -> Self {
        Self { concealed, token }
    }
}

fn clipboard_error(error: arboard::Error) -> Error {
    Error::Other(format!("clipboard: {error}"))
}

#[async_trait]
impl Clipboard for ArboardClipboard {
    fn change_token(&self) -> Option<u64> {
        (self.token)()
    }

    async fn read(&self) -> Result<Option<ClipboardContent>> {
        let concealed = self.concealed;

        tokio::task::spawn_blocking(move || -> Result<Option<ClipboardContent>> {
            if concealed() {
                return Ok(None);
            }

            let mut clipboard = arboard::Clipboard::new().map_err(clipboard_error)?;

            if let Ok(image) = clipboard.get_image() {
                return image_codec::encode_png(image.width, image.height, &image.bytes)
                    .map(|png| Some(ClipboardContent::Png(png)));
            }

            Ok(match clipboard.get_text() {
                Ok(text) if !text.is_empty() => Some(ClipboardContent::Text(text)),
                _ => None,
            })
        })
        .await
        .map_err(|error| Error::Other(error.to_string()))?
    }

    async fn write(&self, content: &ClipboardContent) -> Result<()> {
        let content = content.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut clipboard = arboard::Clipboard::new().map_err(clipboard_error)?;

            match content {
                ClipboardContent::Text(text) => clipboard.set_text(text).map_err(clipboard_error),
                ClipboardContent::Png(png) => {
                    let image = image_codec::decode_png(&png)?;

                    clipboard
                        .set_image(arboard::ImageData {
                            width: image.width,
                            height: image.height,
                            bytes: Cow::Owned(image.bytes),
                        })
                        .map_err(clipboard_error)
                }
            }
        })
        .await
        .map_err(|error| Error::Other(error.to_string()))?
    }
}
