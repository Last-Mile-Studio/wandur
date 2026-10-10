//! Decoding a picture near the size it will be shown at, so a 4000 by 3000 source never becomes a
//! 48 MB bitmap on its way to an 800 pixel plate.
//!
//! - JPEG: `jpeg-decoder` scales while decoding (1/2, 1/4 or 1/8, the smallest at or above the
//!   target); a cover is cropped before the small final resize, so only the shown part is resized.
//!   zune-jpeg (faster at full size, no scaled decoding) and a libjpeg-turbo binding (C build on
//!   every platform) were measured against it in Milestone 4: see docs/measurements.md.
//! - PNG (not interlaced): rows are read one at a time and averaged into a reduced image by an
//!   integer factor, so the full-size bitmap never exists. The C# client decodes PNG whole.
//! - Anything else (interlaced PNG, GIF, WebP, BMP): decoded whole, then resized.
//!
//! The result fits inside the target box (or covers it and is cropped to it, for plates), never
//! upscaled and never stretched.

use image::{RgbaImage, imageops};

/// The box a picture is shown in, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub width: u32,
    pub height: u32,
    /// Scale to cover the box and crop the overflow (a plate), rather than fit inside it.
    pub cover: bool,
}

#[derive(Debug)]
pub struct Decoded {
    pub image: RgbaImage,
    /// The source's size.
    pub source: (u32, u32),
    /// The size the decoder produced before the final resize (the work it did).
    pub decoded: (u32, u32),
}

/// The size the picture is resized to before any crop: the whole picture inside (or covering)
/// the box, never upscaled.
pub fn fitted(source: (u32, u32), target: Target) -> (u32, u32) {
    let (sw, sh) = (f64::from(source.0.max(1)), f64::from(source.1.max(1)));
    let across = f64::from(target.width.max(1)) / sw;
    let down = f64::from(target.height.max(1)) / sh;
    let scale = if target.cover {
        across.max(down)
    } else {
        across.min(down)
    }
    .min(1.0);
    (
        ((sw * scale).round() as u32).max(1),
        ((sh * scale).round() as u32).max(1),
    )
}

pub fn decode_near(bytes: &[u8], target: Target) -> Result<Decoded, String> {
    let decoded = if bytes.starts_with(&[0xFF, 0xD8]) {
        decode_jpeg(bytes, target)?
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_png(bytes, target)?
    } else {
        decode_other(bytes)?
    };
    Ok(finish(decoded, target))
}

/// The final size: the fitted size, cropped to the box for a cover.
fn final_size(source: (u32, u32), target: Target) -> (u32, u32) {
    let (w, h) = fitted(source, target);
    if target.cover {
        (w.min(target.width), h.min(target.height))
    } else {
        (w, h)
    }
}

/// Resize what the decoder produced to the fitted size, then crop a cover to the box (nothing to
/// do when the decoder already produced the final picture, as the JPEG path does).
fn finish(mut d: Decoded, target: Target) -> Decoded {
    if d.image.dimensions() == final_size(d.source, target) {
        return d;
    }
    let (w, h) = fitted(d.source, target);
    if d.image.dimensions() != (w, h) {
        d.image = imageops::resize(&d.image, w, h, imageops::FilterType::Triangle);
    }
    if target.cover && (w > target.width || h > target.height) {
        let cw = w.min(target.width);
        let ch = h.min(target.height);
        let x = (w - cw) / 2;
        let y = (h - ch) / 2;
        d.image = imageops::crop_imm(&d.image, x, y, cw, ch).to_image();
    }
    d
}

fn decode_jpeg(bytes: &[u8], target: Target) -> Result<Decoded, String> {
    let mut decoder = jpeg_decoder::Decoder::new(bytes);
    decoder.read_info().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("no JPEG header")?;
    let source = (u32::from(info.width), u32::from(info.height));
    let (w, h) = fitted(source, target);
    // The decoder picks the smallest of 1/8, 1/4, 1/2 and 1 that is at least this size.
    let (dw, dh) = decoder
        .scale(w.min(u32::from(u16::MAX)) as u16, h.min(u32::from(u16::MAX)) as u16)
        .map_err(|e| e.to_string())?;
    let pixels = decoder.decode().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("no JPEG header")?;
    let (dw, dh) = (u32::from(dw), u32::from(dh));
    let rgb: Vec<u8> = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => pixels,
        jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&l| [l, l, l]).collect(),
        jpeg_decoder::PixelFormat::L16 => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0]])
            .collect(),
        jpeg_decoder::PixelFormat::CMYK32 => pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let k = u16::from(p[3]);
                let c = |v: u8| ((u16::from(v) * k) / 255) as u8;
                [c(p[0]), c(p[1]), c(p[2])]
            })
            .collect(),
    };
    let rgb = image::RgbImage::from_raw(dw, dh, rgb).ok_or("JPEG size mismatch")?;
    // Crop first (a cover keeps only the middle of the fitted picture), then resize only that
    // region, still in RGB: a quarter less work and half the peak of resizing everything.
    let (ow, oh) = final_size(source, target);
    let (cx, cy) = ((w - ow) / 2, (h - oh) / 2);
    let (sx, sy) = (f64::from(dw) / f64::from(w), f64::from(dh) / f64::from(h));
    let rx = ((f64::from(cx) * sx).round() as u32).min(dw - 1);
    let ry = ((f64::from(cy) * sy).round() as u32).min(dh - 1);
    let rw = ((f64::from(ow) * sx).round() as u32).clamp(1, dw - rx);
    let rh = ((f64::from(oh) * sy).round() as u32).clamp(1, dh - ry);
    let view = imageops::crop_imm(&rgb, rx, ry, rw, rh);
    let out = if (rw, rh) == (ow, oh) {
        view.to_image()
    } else {
        imageops::resize(&*view, ow, oh, imageops::FilterType::Triangle)
    };
    drop(rgb);
    let mut rgba = Vec::with_capacity((ow * oh * 4) as usize);
    for p in out.as_raw().as_chunks::<3>().0 {
        rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
    }
    let image = RgbaImage::from_raw(ow, oh, rgba).ok_or("JPEG size mismatch")?;
    Ok(Decoded {
        image,
        source,
        decoded: (dw, dh),
    })
}

fn decode_png(bytes: &[u8], target: Target) -> Result<Decoded, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let info = reader.info();
    let source = (info.width, info.height);
    if info.interlaced {
        return decode_other(bytes);
    }
    let (w, h) = fitted(source, target);
    // Average blocks of factor x factor pixels: the reduced image is still at least the fitted size.
    let factor = (source.0 / w).min(source.1 / h).max(1);
    let (rw, rh) = (source.0 / factor, source.1 / factor);
    let (color, _) = reader.output_color_type();
    let channels = match color {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("indexed PNG not expanded".into()),
    };
    let mut out = Vec::with_capacity((rw * rh * 4) as usize);
    let mut sums = vec![0u32; (rw * 4) as usize];
    let mut rows_in_block = 0;
    let mut y = 0;
    while let Some(row) = reader.next_row().map_err(|e| e.to_string())? {
        if y / factor >= rh {
            break;
        }
        let data = row.data();
        for (x, sum) in sums.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let start = x * factor as usize;
            for px in start..start + factor as usize {
                let p = &data[px * channels..px * channels + channels];
                let (r, g, b, a) = match channels {
                    1 => (p[0], p[0], p[0], 255),
                    2 => (p[0], p[0], p[0], p[1]),
                    3 => (p[0], p[1], p[2], 255),
                    _ => (p[0], p[1], p[2], p[3]),
                };
                sum[0] += u32::from(r);
                sum[1] += u32::from(g);
                sum[2] += u32::from(b);
                sum[3] += u32::from(a);
            }
        }
        rows_in_block += 1;
        y += 1;
        if rows_in_block == factor {
            let n = factor * factor;
            out.extend(sums.iter().map(|s| (s / n) as u8));
            sums.iter_mut().for_each(|s| *s = 0);
            rows_in_block = 0;
        }
    }
    let image = RgbaImage::from_raw(rw, rh, out).ok_or("PNG ended early")?;
    Ok(Decoded {
        image,
        source,
        decoded: (rw, rh),
    })
}

fn decode_other(bytes: &[u8]) -> Result<Decoded, String> {
    let image = image::load_from_memory(bytes).map_err(|e| e.to_string())?.to_rgba8();
    let size = image.dimensions();
    Ok(Decoded {
        image,
        source: size,
        decoded: size,
    })
}

/// Encode a thumbnail for the disk cache (JPEG, quality 85; artwork is opaque).
pub fn encode_thumbnail(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(&rgb)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A test picture with a gradient and some texture.
    pub fn picture(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let n = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) % 64;
            image::Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, (n * 4) as u8, 255])
        })
    }

    pub fn jpeg(w: u32, h: u32) -> Vec<u8> {
        encode_thumbnail(&picture(w, h)).unwrap()
    }

    pub fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(picture(w, h).as_raw(), w, h, image::ExtendedColorType::Rgba8)
            .unwrap();
        out
    }

    use image::ImageEncoder;

    #[test]
    fn fitting_never_upscales_or_stretches() {
        let fit = |s, w, h, cover| {
            fitted(
                s,
                Target {
                    width: w,
                    height: h,
                    cover,
                },
            )
        };
        assert_eq!(fit((4000, 3000), 800, 320, false), (427, 320));
        assert_eq!(fit((4000, 3000), 800, 320, true), (800, 600));
        assert_eq!(fit((400, 300), 800, 320, true), (400, 300), "never upscaled");
        assert_eq!(fit((4000, 3000), 80, 60, false), (80, 60));
    }

    #[test]
    fn jpeg_is_scaled_while_decoding() {
        let bytes = jpeg(1600, 1200);
        let d = decode_near(
            &bytes,
            Target {
                width: 200,
                height: 80,
                cover: true,
            },
        )
        .unwrap();
        assert_eq!(d.source, (1600, 1200));
        assert_eq!(d.decoded, (200, 150), "decoded at 1/8, not at full size");
        assert_eq!(d.image.dimensions(), (200, 80), "cropped to the plate");
        let d = decode_near(
            &bytes,
            Target {
                width: 700,
                height: 700,
                cover: false,
            },
        )
        .unwrap();
        assert_eq!(d.decoded, (800, 600), "1/2 is the smallest at or above the target");
        assert_eq!(d.image.dimensions(), (700, 525));
        // A small picture is not scaled; a cover is still cropped, never stretched.
        let d = decode_near(
            &jpeg(400, 300),
            Target {
                width: 800,
                height: 200,
                cover: true,
            },
        )
        .unwrap();
        assert_eq!(d.decoded, (400, 300));
        assert_eq!(d.image.dimensions(), (400, 200));
        // Cropping before resizing gives the middle of the picture: its centre pixel matches the
        // source's centre.
        let src = picture(1600, 1200);
        let d = decode_near(
            &jpeg(1600, 1200),
            Target {
                width: 200,
                height: 80,
                cover: true,
            },
        )
        .unwrap();
        let (a, b) = (d.image.get_pixel(100, 40), src.get_pixel(800, 600));
        assert!((i32::from(a[0]) - i32::from(b[0])).abs() < 12, "{a:?} {b:?}");
        assert!((i32::from(a[1]) - i32::from(b[1])).abs() < 12, "{a:?} {b:?}");
    }

    #[test]
    fn png_is_reduced_row_by_row() {
        let bytes = png_bytes(1000, 600);
        let d = decode_near(
            &bytes,
            Target {
                width: 100,
                height: 60,
                cover: false,
            },
        )
        .unwrap();
        assert_eq!(d.source, (1000, 600));
        assert_eq!(d.decoded, (100, 60));
        assert_eq!(d.image.dimensions(), (100, 60));
        // The averaged colour of the top-left block is close to the source's.
        let p = d.image.get_pixel(50, 30);
        assert!(
            (i32::from(p[0]) - 127).abs() < 8 && (i32::from(p[1]) - 127).abs() < 8,
            "{p:?}"
        );
    }

    #[test]
    fn other_formats_and_garbage() {
        let mut gif = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut gif)
            .encode(picture(40, 30).as_raw(), 40, 30, image::ExtendedColorType::Rgba8)
            .unwrap();
        let d = decode_near(
            &gif,
            Target {
                width: 20,
                height: 20,
                cover: false,
            },
        )
        .unwrap();
        assert_eq!(d.image.dimensions(), (20, 15));
        assert!(
            decode_near(
                b"<html>not a picture</html>",
                Target {
                    width: 10,
                    height: 10,
                    cover: false
                }
            )
            .is_err()
        );
        assert!(
            decode_near(
                &[0xFF, 0xD8, 0, 0],
                Target {
                    width: 10,
                    height: 10,
                    cover: false
                }
            )
            .is_err()
        );
    }
}
