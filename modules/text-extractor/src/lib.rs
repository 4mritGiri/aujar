//! Text Extractor (PowerToys' Text Extractor): select part of the screen and get the text in
//! it, recognised by Tesseract.
//!
//! [`extract_from_screen`] lets the user drag an area with the desktop's own selection
//! ([`capture`]), enlarges and grays the picture, runs Tesseract on it ([`ocr`]) and tidies
//! the result ([`tidy`]). [`extract_from_file`] does the same for a picture on disk.
//! Copying to the clipboard is left to the caller, which runs in the user's session.

pub mod capture;
pub mod ocr;
mod text;

pub use capture::{Backend, Cancelled};
pub use text::tidy;

use anyhow::{Context, Result};
use async_trait::async_trait;
use aujar_runtime::{Capability, Module, ModuleContext, ModuleMetadata, RuntimeError};
use image::DynamicImage;
use std::path::Path;

const CAPABILITIES: &[Capability] = &[Capability::ScreenCapture, Capability::ExecuteCommand];

static METADATA: ModuleMetadata = ModuleMetadata::new(
    "text_extractor",
    "Text Extractor",
    env!("CARGO_PKG_VERSION"),
    "Select part of the screen and copy the text in it (OCR with Tesseract).",
    CAPABILITIES,
);

/// What to recognise, and how.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Tesseract language codes (`eng`, `nep`); empty means English (or the only installed one).
    pub languages: Vec<String>,
    /// Join the result into one line.
    pub single_line: bool,
    /// How the area is selected on screen.
    pub backend: Backend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// The recognised text, tidied. Empty when the area held no readable text.
    pub text: String,
    /// The languages Tesseract used.
    pub languages: Vec<String>,
}

/// Recognise the text in a picture (blocking).
pub fn extract_from_image(picture: &DynamicImage, options: &Options) -> Result<Extracted> {
    // Check Tesseract and the languages before the user's picture is processed.
    let languages = ocr::resolve_languages(&options.languages, &ocr::languages()?)?;
    let png = ocr::prepare(picture)?;

    // Fully automatic layout first; a small or sparse area can come back empty, so try once
    // more as one uniform block of text.
    let mut raw = ocr::recognize(&png, &languages, 3)?;

    if raw.trim().is_empty() {
        raw = ocr::recognize(&png, &languages, 6)?;
    }

    Ok(Extracted {
        text: tidy(&raw, options.single_line),
        languages,
    })
}

/// Recognise the text in a picture file (PNG, JPEG, WebP, ...). Blocking.
pub fn extract_from_file(path: &Path, options: &Options) -> Result<Extracted> {
    let picture = image::open(path)
        .with_context(|| format!("could not read the picture {}", path.display()))?;

    extract_from_image(&picture, options)
}

/// Let the user select an area of the screen and recognise its text.
///
/// Fails with [`Cancelled`] (find it with `error.downcast_ref`) when the user closes the
/// selection without choosing an area.
pub async fn extract_from_screen(options: Options) -> Result<Extracted> {
    tokio::task::spawn_blocking(move || {
        // Fail before the user drags an area that cannot be read afterwards.
        let languages = ocr::resolve_languages(&options.languages, &ocr::languages()?)?;
        let picture = capture::select_area(options.backend)?;

        extract_from_image(
            &picture,
            &Options {
                languages,
                ..options
            },
        )
    })
    .await
    .context("the text extraction task failed")?
}

/// Registers Text Extractor with the daemon's runtime (`aujar modules`).
#[derive(Debug, Clone, Default)]
pub struct TextExtractorModule;

#[async_trait]
impl Module for TextExtractorModule {
    fn metadata(&self) -> &'static ModuleMetadata {
        &METADATA
    }

    async fn start(&self, _context: ModuleContext) -> Result<(), RuntimeError> {
        // Tesseract is optional: without it the module stays idle and requests explain why.
        match tokio::task::spawn_blocking(ocr::status).await {
            Ok(Ok(status)) => tracing::info!(
                version = %status.version,
                languages = %status.languages.join("+"),
                "Text Extractor ready"
            ),
            _ => tracing::info!("Tesseract is not installed; Text Extractor stays idle"),
        }

        Ok(())
    }

    async fn stop(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}
