//! Decoding a picture within a memory budget (ported from upstream d7429d21, #3644). Every crate that decodes a document's pictures goes
//! through [`probe`]: the header gives the format and size, and the decode is refused before
//! anything is allocated when its peak would pass the budget. The peak counts what a decoder holds
//! beside its output (a progressive JPEG's coefficients, #3644).

use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageFormat};

/// Why a picture isn't decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not a raster picture WordCraft decodes (a metafile, SVG or other bytes).
    NotAPicture,
    /// Its decoder cannot be held to the memory budget.
    Unsupported(&'static str),
    /// Decoding it would take more memory than the budget.
    TooLarge { width: u32, height: u32 },
    /// The header or the data is damaged.
    Broken(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotAPicture => f.write_str("not a supported picture (PNG, JPEG, GIF, WebP, BMP or TIFF)"),
            Refusal::Unsupported(why) => f.write_str(why),
            Refusal::TooLarge { width, height } => write!(f, "the picture is too large ({width} × {height} pixels)"),
            Refusal::Broken(why) => write!(f, "the picture is damaged ({why})"),
        }
    }
}

impl std::error::Error for Refusal {}

fn broken(e: image::ImageError) -> Refusal {
    Refusal::Broken(e.to_string())
}

/// What a picture's header says: its format, its size and the memory decoding it takes. Reading
/// it allocates no pixels.
#[derive(Clone, Copy, Debug)]
pub struct Probe<'a> {
    bytes: &'a [u8],
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    peak: u64,
}

/// Read a picture's header: PNG, JPEG, GIF, WebP, BMP or TIFF.
pub fn probe(bytes: &[u8]) -> Result<Probe<'_>, Refusal> {
    let format = image::guess_format(bytes).map_err(|_| Refusal::NotAPicture)?;
    let held = match format {
        ImageFormat::Png | ImageFormat::Gif | ImageFormat::WebP | ImageFormat::Bmp => 0,
        ImageFormat::Jpeg => jpeg_coefficients(bytes),
        ImageFormat::Tiff => {
            // The TIFF decoder's JPEG strips use independent default limits (#3645).
            // Check before constructing that decoder, including unknown compression kinds.
            if !tiff_compression(bytes).is_some_and(|c| [1, 4, 5, 8, 32946, 32773].contains(&c)) {
                return Err(Refusal::Unsupported("TIFF pictures with JPEG or unknown compression can't be read"));
            }
            0
        }
        _ => return Err(Refusal::NotAPicture),
    };
    let decoder = image::ImageReader::with_format(Cursor::new(bytes), format).into_decoder().map_err(broken)?;
    let (width, height, peak) = measure(&decoder, held)?;
    Ok(Probe { bytes, format, width, height, peak })
}

/// A decoder's size and the peak bytes of decoding it, `held` bytes kept beside its output.
fn measure(decoder: &impl ImageDecoder, held: u64) -> Result<(u32, u32, u64), Refusal> {
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(Refusal::Broken("it has no pixels".into()));
    }
    let native = u64::from(decoder.color_type().bytes_per_pixel());
    let peak = peak_bytes(width, height, native, held).ok_or(Refusal::TooLarge { width, height })?;
    Ok((width, height, peak))
}

impl Probe<'_> {
    /// Bytes decoding takes at its peak.
    pub fn peak_bytes(&self) -> u64 {
        self.peak
    }

    /// Whether decoding fits `budget` bytes.
    pub fn fits(&self, budget: u64) -> bool {
        self.peak <= budget
    }

    /// Decode, refused before anything is allocated when the peak would pass `budget` bytes. The
    /// decoder is held to the header's size and to `budget` as well.
    pub fn decode(&self, budget: u64) -> Result<DynamicImage, Refusal> {
        if !self.fits(budget) {
            return Err(Refusal::TooLarge { width: self.width, height: self.height });
        }
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(self.width);
        limits.max_image_height = Some(self.height);
        limits.max_alloc = Some(budget);
        let mut reader = image::ImageReader::with_format(Cursor::new(self.bytes), self.format);
        reader.limits(limits);
        reader.decode().map_err(broken)
    }
}

/// [`probe`], then [`Probe::decode`] within `budget` bytes.
pub fn decode(bytes: &[u8], budget: u64) -> Result<DynamicImage, Refusal> {
    probe(bytes)?.decode(budget)
}

/// Bytes decoding a `width` × `height` picture holds at its peak: the decoder's output (`native`
/// bytes a pixel) beside what the decoder keeps while it decodes (`held`), then the output beside
/// its RGBA8 conversion, then that beside a premultiplied copy. None on overflow.
fn peak_bytes(width: u32, height: u32, native: u64, held: u64) -> Option<u64> {
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    let decoding = pixels.checked_mul(native)?.checked_add(held)?;
    let converting = pixels.checked_mul(native.checked_add(4)?)?;
    let premultiplying = pixels.checked_mul(8)?;
    Some(decoding.max(converting).max(premultiplying))
}

/// Bytes of DCT coefficients a progressive JPEG's decoder keeps for the whole picture while it
/// reads the scans: zune-jpeg holds an `i16` per coefficient of every component, in whole MCUs.
/// 0 for a baseline JPEG, decoded an MCU row at a time. Each frame header before the first scan
/// counts and the largest wins; headers this can't follow are charged as the costliest frame.
fn jpeg_coefficients(bytes: &[u8]) -> u64 {
    let mut at = 2;
    let mut most = 0u64;
    loop {
        if bytes.get(at) != Some(&0xFF) {
            return worst_coefficients(bytes).max(most);
        }
        while bytes.get(at) == Some(&0xFF) {
            at += 1;
        }
        let Some(&code) = bytes.get(at) else { return worst_coefficients(bytes).max(most) };
        at += 1;
        match code {
            // Markers without a length: start of image, TEM, restarts.
            0xD8 | 0x01 | 0xD0..=0xD7 => continue,
            // End of image, start of scan: every frame header is behind.
            0xD9 | 0xDA => return most,
            _ => {}
        }
        let len = match (bytes.get(at), bytes.get(at + 1)) {
            (Some(&hi), Some(&lo)) => usize::from(u16::from_be_bytes([hi, lo])),
            _ => 0,
        };
        let Some(segment) = bytes.get(at + 2..at + len).filter(|_| len >= 2) else { return worst_coefficients(bytes).max(most) };
        // SOF2, SOF6, SOF10 and SOF14 are the progressive frames.
        if matches!(code, 0xC2 | 0xC6 | 0xCA | 0xCE) {
            let Some(held) = frame_coefficients(segment) else { return worst_coefficients(bytes).max(most) };
            most = most.max(held);
        }
        at += len;
    }
}

/// [`jpeg_coefficients`] of one progressive frame header (after its length): the components'
/// blocks per MCU (horizontal × vertical sampling) times the MCUs that cover the picture.
fn frame_coefficients(sof: &[u8]) -> Option<u64> {
    let height = u64::from(u16::from_be_bytes([*sof.get(1)?, *sof.get(2)?]));
    let width = u64::from(u16::from_be_bytes([*sof.get(3)?, *sof.get(4)?]));
    let n = usize::from(*sof.get(5)?);
    let sampling: Vec<(u64, u64)> =
        sof.get(6..6 + 3 * n)?.as_chunks::<3>().0.iter().map(|&[_, s, _]| (u64::from(s >> 4).clamp(1, 4), u64::from(s & 15).clamp(1, 4))).collect();
    let (h_max, v_max) = sampling.iter().fold((1, 1), |(h, v), &(a, b)| (h.max(a), v.max(b)));
    let mcus = width.div_ceil(8 * h_max).checked_mul(height.div_ceil(8 * v_max))?;
    sampling.iter().try_fold(0u64, |sum, &(h, v)| sum.checked_add(mcus.checked_mul(2 * 64 * h * v)?))
}

/// The coefficients of the costliest progressive frame of the size the image crate reads from
/// `bytes`: four components, each sampled 4 × 4, in MCUs of 32 pixels a side.
fn worst_coefficients(bytes: &[u8]) -> u64 {
    let Ok((w, h)) = image::ImageReader::with_format(Cursor::new(bytes), ImageFormat::Jpeg).into_dimensions() else { return 0 };
    let mcus = u64::from(w).div_ceil(32) * u64::from(h).div_ceil(32);
    mcus.saturating_mul(2 * 64 * 16 * 4)
}

/// Compression of the first classic TIFF image, or 1 when omitted. Refuse malformed,
/// duplicate or unrecognized tags rather than letting a decoder choose a different value.
/// BigTIFF is deliberately refused here: its directory has a different representation.
fn tiff_compression(bytes: &[u8]) -> Option<u16> {
    let le = match bytes.get(..4)? {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let short = |at: usize| -> Option<u16> {
        let b = bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    };
    let long = |at: usize| -> Option<u32> {
        let b = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let ifd = usize::try_from(long(4)?).ok()?;
    let count = usize::from(short(ifd)?);
    let start = ifd.checked_add(2)?;
    bytes.get(start..start.checked_add(count.checked_mul(12)?)?)?;
    let mut compression = None;
    for i in 0..count {
        let entry = start.checked_add(i.checked_mul(12)?)?;
        if short(entry)? != 259 {
            continue;
        }
        if compression.is_some() || long(entry.checked_add(4)?)? != 1 {
            return None;
        }
        compression = Some(match short(entry.checked_add(2)?)? {
            3 => short(entry.checked_add(8)?)?,
            4 => u16::try_from(long(entry.checked_add(8)?)?).ok()?,
            _ => return None,
        });
    }
    Some(compression.unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(img: impl Into<DynamicImage>, format: ImageFormat) -> Vec<u8> {
        let mut out = Vec::new();
        img.into().write_to(&mut Cursor::new(&mut out), format).unwrap();
        out
    }

    /// A PNG whose header declares `w` × `h` pixels, with no pixel data.
    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut png = encode(image::RgbaImage::new(1, 1), ImageFormat::Png);
        png[16..20].copy_from_slice(&w.to_be_bytes());
        png[20..24].copy_from_slice(&h.to_be_bytes());
        // The IHDR CRC covers the chunk type and data.
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        png
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 == 1 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }

    #[test]
    fn small_pictures_decode_and_peaks_count_every_buffer() {
        let png = encode(image::RgbaImage::from_pixel(16, 8, image::Rgba([1, 2, 3, 4])), ImageFormat::Png);
        let p = probe(&png).unwrap();
        assert_eq!((p.format, p.width, p.height), (ImageFormat::Png, 16, 8));
        assert_eq!(p.peak_bytes(), 16 * 8 * 8, "RGBA8: the decode beside its premultiplied copy");
        assert!(p.fits(1024) && !p.fits(1023));
        assert_eq!(p.decode(1 << 20).unwrap().to_rgba8().get_pixel(3, 3).0, [1, 2, 3, 4]);
        assert_eq!(p.decode(1023), Err(Refusal::TooLarge { width: 16, height: 8 }));
        assert_eq!(peak_bytes(16, 8, 8, 0), Some(16 * 8 * 12), "RGBA16 beside its RGBA8 conversion");
        assert_eq!(peak_bytes(16, 8, 3, 16 * 8 * 8), Some(16 * 8 * 11), "coefficients beside the RGB output");
        assert_eq!(peak_bytes(u32::MAX, u32::MAX, 8, 0), None);
        for bytes in [&b"not a picture"[..], b"", b"\x89PNG"] {
            assert!(probe(bytes).is_err(), "{bytes:?}");
        }
    }

    #[test]
    fn a_huge_header_is_refused_before_decoding() {
        let png = png_header(60_000, 60_000);
        let p = probe(&png).unwrap();
        assert_eq!((p.width, p.height), (60_000, 60_000));
        assert!(!p.fits(crate::MAX_DECODED_BYTES));
        assert_eq!(decode(&png, crate::MAX_DECODED_BYTES).unwrap_err(), Refusal::TooLarge { width: 60_000, height: 60_000 });
    }

    /// A baseline JPEG turned progressive in its headers (SOF2, a DC-only first scan), `w` × `h`
    /// with its three components unsubsampled.
    fn progressive_header(w: u16, h: u16) -> Vec<u8> {
        let mut jpeg = encode(image::RgbImage::from_pixel(16, 16, image::Rgb([200, 30, 30])), ImageFormat::Jpeg);
        let sof = jpeg.windows(2).position(|m| m == [0xFF, 0xC0]).unwrap();
        jpeg[sof + 1] = 0xC2;
        jpeg[sof + 5..sof + 7].copy_from_slice(&h.to_be_bytes());
        jpeg[sof + 7..sof + 9].copy_from_slice(&w.to_be_bytes());
        assert_eq!(jpeg[sof + 9], 3);
        for c in 0..3 {
            jpeg[sof + 11 + 3 * c] = 0x11;
        }
        let sos = jpeg.windows(2).position(|m| m == [0xFF, 0xDA]).unwrap();
        let n = usize::from(jpeg[sos + 4]);
        jpeg[sos + 6 + 2 * n] = 0;
        jpeg
    }

    #[test]
    fn progressive_jpeg_coefficients_are_charged() {
        let jpeg = progressive_header(2048, 2048);
        let p = probe(&jpeg).unwrap();
        assert_eq!((p.width, p.height), (2048, 2048));
        // RGB output (3 bytes a pixel) beside 2 bytes a coefficient of three full-size components:
        // 9 bytes a pixel, more than the 8 two RGBA buffers take.
        assert_eq!(p.peak_bytes(), 2048 * 2048 * 9);
        assert!(!p.fits(2048 * 2048 * 8));
        // Baseline: decoded an MCU row at a time, nothing held.
        let baseline = encode(image::RgbImage::new(64, 64), ImageFormat::Jpeg);
        assert_eq!(probe(&baseline).unwrap().peak_bytes(), 64 * 64 * 8);
    }

    #[test]
    fn coefficients_follow_sampling_and_mcu_padding() {
        // P, height, width, components (id, sampling, table).
        let sof = |w: u16, h: u16, comps: &[u8]| {
            let mut s = vec![8];
            s.extend(h.to_be_bytes());
            s.extend(w.to_be_bytes());
            s.push((comps.len() / 3) as u8);
            s.extend(comps);
            s
        };
        // Four components, no subsampling: 2 bytes a coefficient, 8 a pixel.
        assert_eq!(frame_coefficients(&sof(64, 64, &[1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0, 4, 0x11, 0])), Some(64 * 64 * 8));
        // 4:2:0: MCUs of 16 × 16 holding 4 + 1 + 1 blocks; a 1-pixel-wide picture still takes one.
        assert_eq!(frame_coefficients(&sof(1, 16, &[1, 0x22, 0, 2, 0x11, 0, 3, 0x11, 0])), Some(128 * 6));
        let mut short = sof(1, 16, &[1, 0x22, 0, 2, 0x11, 0]);
        short[5] = 3;
        assert_eq!(frame_coefficients(&short), None, "three components declared, two there");
        // A marker walk over SOI, an APP0 segment, SOF2, then the scan.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC2];
        let f = sof(64, 64, &[1, 0x11, 0]);
        jpeg.extend(((f.len() + 2) as u16).to_be_bytes());
        jpeg.extend(&f);
        jpeg.extend([0xFF, 0xDA]);
        assert_eq!(jpeg_coefficients(&jpeg), 64 * 64 * 2);
    }
    fn compression_header(values: &[u16]) -> Vec<u8> {
        let mut bytes = b"II*\0".to_vec();
        bytes.extend(8u32.to_le_bytes());
        bytes.extend((values.len() as u16).to_le_bytes());
        for value in values {
            bytes.extend(259u16.to_le_bytes());
            bytes.extend(3u16.to_le_bytes());
            bytes.extend(1u32.to_le_bytes());
            bytes.extend(u32::from(*value).to_le_bytes());
        }
        bytes.extend(0u32.to_le_bytes());
        bytes
    }

    #[test]
    fn tiff_jpeg_and_unknown_compressions_are_refused_before_decoder_creation() {
        for value in [6, 7, 999] {
            assert!(matches!(probe(&compression_header(&[value])), Err(Refusal::Unsupported(_))), "compression {value}");
        }
        let be = b"MM\0*\0\0\0\x08\0\x01\x01\x03\0\x03\0\0\0\x01\0\x07\0\0";
        assert_eq!(tiff_compression(be), Some(7));
        assert!(matches!(probe(be), Err(Refusal::Unsupported(_))));
        assert_eq!(tiff_compression(&compression_header(&[])), Some(1));
        assert_eq!(tiff_compression(&compression_header(&[1, 7])), None, "a duplicate tag cannot hide JPEG");
        assert_eq!(tiff_compression(&compression_header(&[7])[..12]), None);
        let tiff = encode(image::RgbImage::from_pixel(5, 3, image::Rgb([9, 8, 7])), ImageFormat::Tiff);
        assert_eq!(decode(&tiff, 1 << 20).unwrap().to_rgb8().get_pixel(4, 2).0, [9, 8, 7]);
    }
}
