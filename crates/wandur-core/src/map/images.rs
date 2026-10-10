//! Pictures for map labels: PNG or JPEG bytes keyed by their SHA-256, at most
//! [`MAX_IMAGE_BYTES`] each and [`MAX_IMAGES_BYTES`] for a whole map. A picture brought in from
//! outside (a file the person picked, a Mudlet label) goes through [`prepare`]: it must decode,
//! and one that is too large is scaled down (and encoded again) until it fits.
//!
//! Map files carry the bytes as base64 ([`encode_base64`], [`decode_base64`]: the standard
//! alphabet; reading ignores whitespace and missing padding).

use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::model::MapImage;

/// Largest picture, after scaling down (2 MiB).
pub const MAX_IMAGE_BYTES: usize = 2 * 1024 * 1024;
/// All the pictures of one map (20 MiB).
pub const MAX_IMAGES_BYTES: usize = 20 * 1024 * 1024;
/// Most labels in a map.
pub const MAX_LABELS: usize = 5_000;
/// A picture larger than this on either side is scaled down to it.
pub const MAX_SIDE: u32 = 2048;
/// Pictures larger than this on either side are not read at all (a guard against bombs).
const READ_LIMIT: u32 = 16_384;

/// Why a picture was not taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// Not PNG or JPEG, or the bytes do not decode.
    Unreadable,
    /// Still over [`MAX_IMAGE_BYTES`] after scaling down, or too large to read.
    TooLarge,
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::l10n::{S, t};
        f.write_str(match self {
            ImageError::Unreadable => t(S::MapImageUnreadable),
            ImageError::TooLarge => t(S::MapImageTooLarge),
        })
    }
}

impl std::error::Error for ImageError {}

/// The kind of picture the bytes hold, by their first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
}

impl ImageKind {
    pub fn of(data: &[u8]) -> Option<ImageKind> {
        if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(ImageKind::Png)
        } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(ImageKind::Jpeg)
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ImageKind::Png => "PNG",
            ImageKind::Jpeg => "JPEG",
        }
    }

    fn format(self) -> image::ImageFormat {
        match self {
            ImageKind::Png => image::ImageFormat::Png,
            ImageKind::Jpeg => image::ImageFormat::Jpeg,
        }
    }
}

/// The SHA-256 of the bytes, lowercase hex.
pub fn hash(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Whether `text` looks like a picture hash (64 lowercase hex digits).
pub fn is_hash(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A picture as stored, unchanged (the caller has checked it).
pub fn image(data: impl Into<Arc<[u8]>>) -> MapImage {
    let data: Arc<[u8]> = data.into();
    MapImage {
        hash: hash(&data),
        data,
    }
}

/// Whether a stored picture is acceptable without decoding it: a known kind, within the size
/// limit, its hash its bytes'.
pub fn valid_image(image: &MapImage) -> bool {
    !image.data.is_empty()
        && image.data.len() <= MAX_IMAGE_BYTES
        && ImageKind::of(&image.data).is_some()
        && is_hash(&image.hash)
        && hash(&image.data) == image.hash
}

/// A picture's size in pixels, read from its header (no decoding).
pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let kind = ImageKind::of(data)?;
    image::ImageReader::with_format(std::io::Cursor::new(data), kind.format())
        .into_dimensions()
        .ok()
}

fn reader(data: &[u8], kind: ImageKind) -> image::ImageReader<std::io::Cursor<&[u8]>> {
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(data), kind.format());
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(READ_LIMIT);
    limits.max_image_height = Some(READ_LIMIT);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    reader
}

/// Take a picture from outside: PNG or JPEG that decodes. One over [`MAX_IMAGE_BYTES`] or
/// [`MAX_SIDE`] is scaled down (PNG stays PNG, JPEG stays JPEG) until it fits.
pub fn prepare(data: &[u8]) -> Result<MapImage, ImageError> {
    let kind = ImageKind::of(data).ok_or(ImageError::Unreadable)?;
    let (w, h) = dimensions(data).ok_or(ImageError::Unreadable)?;
    if w == 0 || h == 0 {
        return Err(ImageError::Unreadable);
    }
    if w > READ_LIMIT || h > READ_LIMIT {
        return Err(ImageError::TooLarge);
    }
    let decoded = reader(data, kind).decode().map_err(|e| match e {
        image::ImageError::Limits(_) => ImageError::TooLarge,
        _ => ImageError::Unreadable,
    })?;
    if data.len() <= MAX_IMAGE_BYTES && w <= MAX_SIDE && h <= MAX_SIDE {
        return Ok(image(data.to_vec()));
    }
    let mut side = MAX_SIDE.min(w.max(h));
    loop {
        let scaled = decoded.resize(side, side, image::imageops::FilterType::Triangle);
        let mut out = Vec::new();
        let encoded = match kind {
            ImageKind::Png => scaled.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png),
            ImageKind::Jpeg => {
                let rgb = scaled.to_rgb8();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&rgb)
            }
        };
        encoded.map_err(|_| ImageError::Unreadable)?;
        if out.len() <= MAX_IMAGE_BYTES {
            return Ok(image(out));
        }
        if side <= 64 {
            return Err(ImageError::TooLarge);
        }
        side = (side * 3 / 4).max(64);
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 with the standard alphabet and padding.
pub fn encode_base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Read base64 (standard or URL-safe alphabet); whitespace is skipped and padding optional.
/// `None` for anything else, or when the result would pass `limit` bytes.
pub fn decode_base64(text: &str, limit: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity((text.len() / 4 * 3).min(limit));
    let mut acc: u32 = 0;
    let mut bits = 0;
    let mut ended = false;
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => {
                ended = true;
                continue;
            }
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        if ended {
            return None;
        }
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
            if out.len() > limit {
                return None;
            }
        }
    }
    Some(out)
}

pub(crate) fn serialize_bytes<S: serde::Serializer>(data: &Arc<[u8]>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&encode_base64(data))
}

pub(crate) fn deserialize_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Arc<[u8]>, D::Error> {
    let text = <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(d)?;
    decode_base64(&text, MAX_IMAGE_BYTES)
        .map(Arc::from)
        .ok_or_else(|| serde::de::Error::custom("not a picture"))
}

/// A PNG made here for tests: `w` by `h`, a gradient.
#[cfg(test)]
pub(crate) fn test_png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 90, 255]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        test_png(w, h)
    }

    #[test]
    fn base64_round_trips_and_reads_loosely() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
            let text = encode_base64(data);
            assert_eq!(decode_base64(&text, 100).unwrap(), data);
        }
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(decode_base64("Zm9v\nYmE", 100).unwrap(), b"fooba");
        assert_eq!(decode_base64("Zm8", 100).unwrap(), b"fo");
        assert!(decode_base64("Zm9v*", 100).is_none());
        assert!(decode_base64("Zm9vYmFy", 4).is_none());
    }

    #[test]
    fn a_small_picture_is_kept_as_it_is_and_hashed() {
        let data = png(8, 4);
        let image = prepare(&data).unwrap();
        assert_eq!(&*image.data, &data[..]);
        assert!(valid_image(&image));
        assert_eq!(dimensions(&image.data), Some((8, 4)));
        assert_eq!(image.hash, hash(&data));
        assert!(is_hash(&image.hash));
    }

    #[test]
    fn a_large_picture_is_scaled_down_and_junk_is_refused() {
        let data = png(3000, 1000);
        let image = prepare(&data).unwrap();
        assert_eq!(dimensions(&image.data), Some((MAX_SIDE, 683)));
        assert!(image.data.len() <= MAX_IMAGE_BYTES);
        assert_eq!(prepare(b"GIF89a....").unwrap_err(), ImageError::Unreadable);
        let mut broken = png(4, 4);
        broken.truncate(30);
        assert_eq!(prepare(&broken).unwrap_err(), ImageError::Unreadable);
        let tampered = MapImage {
            hash: "0".repeat(64),
            data: Arc::from(png(2, 2)),
        };
        assert!(!valid_image(&tampered));
    }
}
