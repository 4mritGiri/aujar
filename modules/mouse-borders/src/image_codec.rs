//! PNG encoding and decoding for the clipboard.
//!
//! Images travel between machines as PNG. Windows and macOS clipboards hand over raw RGBA
//! pixels, so these functions convert. Decoding is bounded: a hostile peer cannot make us
//! allocate gigabytes with a small, highly compressed image.

use std::io::Cursor;

use crate::error::{Error, Result};

/// Largest width or height accepted.
const MAX_SIDE: u32 = 16_384;

/// Largest decoded size accepted (RGBA bytes).
const MAX_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    /// `width * height * 4` bytes, row by row.
    pub bytes: Vec<u8>,
}

fn invalid(what: impl std::fmt::Display) -> Error {
    Error::Other(format!("invalid image: {what}"))
}

pub fn encode_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>> {
    if width == 0 || height == 0 || rgba.len() != width * height * 4 {
        return Err(invalid("the pixel data does not match the size"));
    }

    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);

    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);

    let mut writer = encoder.write_header().map_err(invalid)?;

    writer.write_image_data(rgba).map_err(invalid)?;
    writer.finish().map_err(invalid)?;

    Ok(out)
}

pub fn decode_png(data: &[u8]) -> Result<Rgba> {
    let mut decoder = png::Decoder::new(Cursor::new(data));

    // Palettes and low bit depths become plain 8-bit channels; 16-bit is reduced to 8.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);

    let mut reader = decoder.read_info().map_err(invalid)?;
    let (width, height) = {
        let info = reader.info();

        (info.width, info.height)
    };

    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(invalid("the dimensions are out of range"));
    }

    if reader.output_buffer_size() > MAX_BYTES {
        return Err(invalid("the image is too large"));
    }

    let mut buffer = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buffer).map_err(invalid)?;
    let pixels = &buffer[..frame.buffer_size()];

    let bytes = match frame.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err(invalid("an unexpanded palette")),
    };

    Ok(Rgba {
        width: width as usize,
        height: height as usize,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: usize, height: usize) -> Vec<u8> {
        (0..width * height)
            .flat_map(|i| {
                [
                    (i % 251) as u8,
                    (i % 13) as u8,
                    (i % 7) as u8,
                    255 - (i % 5) as u8,
                ]
            })
            .collect()
    }

    #[test]
    fn pixels_survive_a_round_trip() {
        let pixels = gradient(37, 19);
        let png = encode_png(37, 19, &pixels).unwrap();

        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(
            decode_png(&png).unwrap(),
            Rgba {
                width: 37,
                height: 19,
                bytes: pixels
            }
        );
    }

    #[test]
    fn mismatched_or_empty_data_is_refused() {
        assert!(encode_png(2, 2, &[0; 15]).is_err());
        assert!(encode_png(0, 2, &[]).is_err());
        assert!(encode_png(2, 0, &[]).is_err());
    }

    #[test]
    fn rgb_and_grayscale_images_become_rgba() {
        let mut rgb = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut rgb, 2, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[10, 20, 30, 40, 50, 60])
                .unwrap();
        }
        let mut gray = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut gray, 2, 1);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[7, 9])
                .unwrap();
        }

        assert_eq!(
            decode_png(&rgb).unwrap().bytes,
            [10, 20, 30, 255, 40, 50, 60, 255]
        );
        assert_eq!(
            decode_png(&gray).unwrap().bytes,
            [7, 7, 7, 255, 9, 9, 9, 255]
        );
    }

    #[test]
    fn garbage_and_truncated_data_are_rejected() {
        assert!(decode_png(b"").is_err());
        assert!(decode_png(b"not a png at all").is_err());

        let png = encode_png(8, 8, &gradient(8, 8)).unwrap();

        assert!(decode_png(&png[..png.len() / 2]).is_err());
    }

    #[test]
    fn absurd_dimensions_are_refused_before_allocating() {
        // A real PNG header claiming 100000 x 100000 pixels, with no pixel data.
        let mut huge = Vec::new();
        {
            let encoder = {
                let mut e = png::Encoder::new(&mut huge, 100_000, 100_000);
                e.set_color(png::ColorType::Grayscale);
                e.set_depth(png::BitDepth::One);
                e
            };
            let _ = encoder.write_header();
        }

        assert!(decode_png(&huge).is_err());
    }
}
