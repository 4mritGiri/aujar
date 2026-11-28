//! Text Extractor (PowerToys' Text Extractor): select part of the screen and get the text in
//! it, recognised by the operating system's OCR ([`native`]: Windows OCR, Apple Vision) or by
//! Tesseract (Linux, and languages the native engine does not read).
//!
//! [`extract_from_screen`] lets the user drag an area with the desktop's own selection
//! ([`capture`]), enlarges and grays the picture, runs Tesseract on it ([`ocr`]) and tidies
//! the result ([`tidy`]). [`extract_from_file`] does the same for a picture on disk.
//! Copying to the clipboard is left to the caller, which runs in the user's session.

pub mod native;
pub mod ocr;
pub mod redact;
mod text;

/// Screen capture, window positions and the capture permission now live in `aujar-screen`;
/// they are re-exported here for existing callers.
pub use aujar_screen::{Backend, Cancelled, capture, permission, windows};
pub use native::Engine;
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
    /// Which engine reads the text.
    pub engine: Engine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// The recognised text, tidied. Empty when the area held no readable text.
    pub text: String,
    /// The languages the engine used (Tesseract codes, or the native engine's tags).
    pub languages: Vec<String>,
    /// `Tesseract`, `Windows OCR` or `Apple Vision`.
    pub engine: &'static str,
}

fn native_unavailable(languages: &[String]) -> anyhow::Error {
    anyhow::anyhow!(
        "{} cannot read {}; set `[text_extractor] engine = \"auto\"` or \"tesseract\"",
        native::native_name().unwrap_or("This platform has no built-in OCR and"),
        if languages.is_empty() {
            "English".to_owned()
        } else {
            languages.join("+")
        }
    )
}

/// The words in `picture` with their boxes in `picture` pixels: the native engine when `engine`
/// allows and it reads the languages, else Tesseract. Also the languages and engine used.
fn words_in(
    picture: &image::RgbaImage,
    languages: &[String],
    engine: Engine,
) -> Result<(Vec<ocr::Word>, Vec<String>, &'static str)> {
    if engine != Engine::Tesseract {
        match native::recognize(picture, languages) {
            Ok(Some((words, used))) => {
                return Ok((words, used, native::native_name().unwrap_or("native")));
            }
            Ok(None) if engine == Engine::Native => return Err(native_unavailable(languages)),
            Err(error) if engine == Engine::Native => return Err(error),
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "native OCR failed; using Tesseract");
            }
            Ok(None) => {}
        }
    }

    let languages = ocr::resolve_languages(languages, &ocr::languages()?)?;
    let dynamic = DynamicImage::ImageRgba8(picture.clone());
    let (png, factor) = ocr::prepare_scaled(&dynamic)?;
    let mut words = ocr::recognize_words(&png, &languages, 3)?;

    if words.is_empty() {
        // Sparse text: find as much as possible in no particular order.
        words = ocr::recognize_words(&png, &languages, 11)?;
    }

    // Back from the enlarged picture's pixels.
    for word in &mut words {
        word.left /= factor;
        word.top /= factor;
        word.width = word.width.div_ceil(factor);
        word.height = word.height.div_ceil(factor);
    }

    Ok((words, languages, "Tesseract"))
}

/// Recognise the text in a picture (blocking).
pub fn extract_from_image(picture: &DynamicImage, options: &Options) -> Result<Extracted> {
    if options.engine != Engine::Tesseract {
        match native::recognize(&picture.to_rgba8(), &options.languages) {
            Ok(Some((words, languages))) => {
                return Ok(Extracted {
                    text: tidy(&native::words_to_text(&words), options.single_line),
                    languages,
                    engine: native::native_name().unwrap_or("native"),
                });
            }
            Ok(None) if options.engine == Engine::Native => {
                return Err(native_unavailable(&options.languages));
            }
            Err(error) if options.engine == Engine::Native => return Err(error),
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "native OCR failed; using Tesseract");
            }
            Ok(None) => {}
        }
    }

    // Tesseract's own text output keeps its layout better than words joined back together.
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
        engine: "Tesseract",
    })
}

/// Cover what looks like secrets or personal data in `picture` ([`redact`]). Returns the
/// covered copy and how many words were covered. Blocking.
pub fn redact_image(
    picture: &image::RgbaImage,
    languages: &[String],
    engine: Engine,
) -> Result<(image::RgbaImage, usize)> {
    let (words, _, _) = words_in(picture, languages, engine)?;
    let covers = redact::covers(&words);
    let mut covered = picture.clone();
    let (width, height) = covered.dimensions();

    for cover in &covers {
        let left = cover.left.min(width);
        let top = cover.top.min(height);
        let right = (cover.left + cover.width).min(width);
        let bottom = (cover.top + cover.height).min(height);

        for y in top..bottom {
            for x in left..right {
                covered.put_pixel(x, y, image::Rgba([17, 17, 17, 255]));
            }
        }
    }

    Ok((covered, covers.len()))
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
