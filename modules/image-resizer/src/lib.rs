//! A portable, filesystem-only Image Resizer engine.

use image::{DynamicImage, ImageFormat, imageops::FilterType};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeOptions {
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Do not enlarge an image that already fits within the requested bounds.
    pub shrink_only: bool,
}

impl ResizeOptions {
    pub fn validate(self) -> Result<Self, ResizeError> {
        if self.width == Some(0) || self.height == Some(0) {
            return Err(ResizeError::ZeroDimension);
        }
        if self.width.is_none() && self.height.is_none() {
            return Err(ResizeError::MissingDimensions);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResizeResult {
    pub input: PathBuf,
    pub output: PathBuf,
    pub source_width: u32,
    pub source_height: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Error)]
pub enum ResizeError {
    #[error("provide --width, --height, or both")]
    MissingDimensions,
    #[error("image dimensions must be greater than zero")]
    ZeroDimension,
    #[error("cannot determine an output image format from `{0}")]
    UnknownOutputFormat(PathBuf),
    #[error("failed to read image `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("failed to write image `{path}`: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
}

/// Resize one image, preserving aspect ratio. When both dimensions are supplied, the image fits
/// inside that box (it is never cropped or distorted).
pub fn resize(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: ResizeOptions,
) -> Result<ResizeResult, ResizeError> {
    let options = options.validate()?;
    let input = input.as_ref().to_path_buf();
    let output = output.as_ref().to_path_buf();
    let image = image::open(&input).map_err(|source| ResizeError::Read {
        path: input.clone(),
        source,
    })?;
    let (source_width, source_height) = (image.width(), image.height());
    let (width, height) = target_size(source_width, source_height, options);
    let resized = if width == source_width && height == source_height {
        image
    } else {
        image.resize_exact(width, height, FilterType::Lanczos3)
    };
    write(resized, &output)?;
    Ok(ResizeResult {
        input,
        output,
        source_width,
        source_height,
        width,
        height,
    })
}

pub fn target_size(source_width: u32, source_height: u32, options: ResizeOptions) -> (u32, u32) {
    let options = options.validate().expect("validated resize options");
    let (max_width, max_height) = match (options.width, options.height) {
        (Some(width), Some(height)) => (width, height),
        (Some(width), None) => (width, u32::MAX),
        (None, Some(height)) => (u32::MAX, height),
        (None, None) => unreachable!(),
    };
    let scale =
        (max_width as f64 / source_width as f64).min(max_height as f64 / source_height as f64);
    let scale = if options.shrink_only {
        scale.min(1.0)
    } else {
        scale
    };
    (
        (source_width as f64 * scale).round().max(1.0) as u32,
        (source_height as f64 * scale).round().max(1.0) as u32,
    )
}

fn write(image: DynamicImage, output: &Path) -> Result<(), ResizeError> {
    let format = ImageFormat::from_path(output)
        .map_err(|_| ResizeError::UnknownOutputFormat(output.to_path_buf()))?;
    image
        .save_with_format(output, format)
        .map_err(|source| ResizeError::Write {
            path: output.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_box_without_distortion() {
        assert_eq!(
            target_size(
                400,
                200,
                ResizeOptions {
                    width: Some(100),
                    height: Some(100),
                    shrink_only: false
                }
            ),
            (100, 50)
        );
    }

    #[test]
    fn one_dimension_preserves_aspect_ratio() {
        assert_eq!(
            target_size(
                160,
                80,
                ResizeOptions {
                    width: None,
                    height: Some(90),
                    shrink_only: false
                }
            ),
            (180, 90)
        );
    }

    #[test]
    fn shrink_only_never_upscales() {
        assert_eq!(
            target_size(
                100,
                50,
                ResizeOptions {
                    width: Some(200),
                    height: None,
                    shrink_only: true
                }
            ),
            (100, 50)
        );
    }
}
