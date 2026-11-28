//! Apple Vision (`VNRecognizeTextRequest`): accurate recognition with language correction, and a
//! box for every word (`boundingBoxForRange`).

use crate::ocr::Word;
use anyhow::{Context, Result, bail};
use objc2::{AnyThread, rc::Retained};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSRange, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

/// The requested tags Vision supports (`en` matches `en-US`), as Vision names them.
fn supported(request: &VNRecognizeTextRequest, tags: &[&str]) -> Vec<Retained<NSString>> {
    // SAFETY: a plain query on a fresh request.
    let Ok(available) = (unsafe { request.supportedRecognitionLanguagesAndReturnError() }) else {
        return Vec::new();
    };

    let available: Vec<String> = available.iter().map(|tag| tag.to_string()).collect();

    tags.iter()
        .filter_map(|tag| {
            available
                .iter()
                .find(|known| *known == tag || known.starts_with(&format!("{tag}-")))
                .map(|known| NSString::from_str(known))
        })
        .collect()
}

pub(super) fn recognize(
    picture: &image::RgbaImage,
    tags: &[&str],
) -> Result<Option<(Vec<Word>, Vec<String>)>> {
    let (width, height) = (picture.width() as f64, picture.height() as f64);

    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(picture.clone())
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .context("could not encode the picture for Vision")?;

    let request = VNRecognizeTextRequest::new();
    let languages = supported(&request, tags);

    // All or nothing, as with the tags themselves.
    if languages.len() != tags.len() {
        return Ok(None);
    }

    let used: Vec<String> = languages.iter().map(|tag| tag.to_string()).collect();

    request.setRecognitionLanguages(&NSArray::from_retained_slice(&languages));
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);

    let data = NSData::with_bytes(&png);
    let handler = VNImageRequestHandler::initWithData_options(
        VNImageRequestHandler::alloc(),
        &data,
        &NSDictionary::new(),
    );
    let as_request: Retained<VNRequest> =
        Retained::into_super(Retained::into_super(request.clone()));

    if let Err(error) = handler.performRequests_error(&NSArray::from_retained_slice(&[as_request]))
    {
        bail!(
            "Vision could not read the picture: {}",
            error.localizedDescription()
        );
    }

    let mut words = Vec::new();

    for (index, observation) in request.results().into_iter().flatten().enumerate() {
        let candidates = observation.topCandidates(1);
        let Some(best) = candidates.firstObject() else {
            continue;
        };
        let text = best.string().to_string();

        // Each word's own box, found by its range in the line (UTF-16 offsets).
        let mut offset = 0usize;

        for piece in text.split(' ') {
            let length = piece.encode_utf16().count();

            if !piece.trim().is_empty() {
                // SAFETY: the range lies inside the candidate's string.
                let rect = unsafe { best.boundingBoxForRange_error(NSRange::new(offset, length)) }
                    .ok()
                    .map(|observation| unsafe { observation.boundingBox() });

                if let Some(rect) = rect {
                    // Normalized, origin at the bottom left.
                    let left = rect.origin.x * width;
                    let top = (1.0 - rect.origin.y - rect.size.height) * height;

                    words.push(Word {
                        left: left.max(0.0) as u32,
                        top: top.max(0.0) as u32,
                        width: (rect.size.width * width).ceil() as u32,
                        height: (rect.size.height * height).ceil() as u32,
                        line: (0, 0, index as u32),
                        text: piece.to_owned(),
                    });
                }
            }

            offset += length + 1;
        }
    }

    Ok(Some((words, used)))
}
