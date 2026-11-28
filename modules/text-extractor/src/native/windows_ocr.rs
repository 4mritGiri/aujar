//! Windows.Media.Ocr: the OCR in Windows 10 and 11. It reads one language per engine, and only
//! languages whose OCR pack is installed (Settings > Time & language > Language).

use crate::ocr::{Word, scale_factor};
use anyhow::{Context, Result};
use image::imageops::FilterType;
use windows::{
    Globalization::Language,
    Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap},
    Media::Ocr::OcrEngine,
    Security::Cryptography::CryptographicBuffer,
    core::HSTRING,
};

fn supported(tag: &str) -> Option<Language> {
    let language = Language::CreateLanguage(&HSTRING::from(tag)).ok()?;

    OcrEngine::IsLanguageSupported(&language)
        .ok()
        .filter(|supported| *supported)
        .map(|_| language)
}

pub(super) fn recognize(
    picture: &image::RgbaImage,
    tags: &[&str],
) -> Result<Option<(Vec<Word>, Vec<String>)>> {
    // One engine reads one language: the first requested one Windows can read.
    let Some((tag, language)) = tags
        .iter()
        .find_map(|tag| supported(tag).map(|language| (*tag, language)))
    else {
        return Ok(None);
    };

    let engine = OcrEngine::TryCreateFromLanguage(&language)
        .context("Windows could not start its OCR engine")?;

    // Small screen text reads better enlarged, within the engine's limit.
    let (width, height) = picture.dimensions();
    let limit = OcrEngine::MaxImageDimension().unwrap_or(2600).max(1);
    let mut factor = scale_factor(width, height);

    while factor > 1 && (width * factor > limit || height * factor > limit) {
        factor -= 1;
    }

    let shrink = (width.max(height) > limit).then(|| limit as f32 / width.max(height) as f32);
    let prepared = match (factor, shrink) {
        (_, Some(ratio)) => image::imageops::resize(
            picture,
            ((width as f32 * ratio) as u32).max(1),
            ((height as f32 * ratio) as u32).max(1),
            FilterType::Triangle,
        ),
        (1, None) => picture.clone(),
        (factor, None) => image::imageops::resize(
            picture,
            width * factor,
            height * factor,
            FilterType::CatmullRom,
        ),
    };
    // Picture pixels per prepared pixel, to put the boxes back.
    let back = width as f32 / prepared.width() as f32;

    let mut bgra = prepared.clone().into_raw();

    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    let buffer = CryptographicBuffer::CreateFromByteArray(&bgra)?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        prepared.width() as i32,
        prepared.height() as i32,
        BitmapAlphaMode::Premultiplied,
    )?;

    let result = engine
        .RecognizeAsync(&bitmap)?
        .join()
        .context("Windows OCR failed")?;

    let mut words = Vec::new();

    for (index, line) in result.Lines()?.into_iter().enumerate() {
        for word in line.Words()? {
            let rect = word.BoundingRect()?;
            let text = word.Text()?.to_string_lossy();

            if text.trim().is_empty() {
                continue;
            }

            words.push(Word {
                left: (rect.X * back).max(0.0) as u32,
                top: (rect.Y * back).max(0.0) as u32,
                width: (rect.Width * back).ceil() as u32,
                height: (rect.Height * back).ceil() as u32,
                // Windows OCR has lines, not blocks.
                line: (0, 0, index as u32),
                text,
            });
        }
    }

    Ok(Some((words, vec![tag.to_owned()])))
}
