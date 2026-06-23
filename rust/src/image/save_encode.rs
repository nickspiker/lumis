//! RGB export encoding for the save pipeline (JPEG / TIFF / JPEG XL).
//!
//! The camera thread hands us the displayed slot's raw bayer data; we debayer it (the same 2x2-block approach the live preview uses), apply the XYZ colour matrix + sqrt encode, then encode to the requested container as bytes for Kotlin to write via MediaStore. DNG is built separately (chameleon::make_base_dng + raw bayer); this module is RGB-only.

use std::io::Cursor;

/// Debayer + colour-correct the average slot into 8-bit RGB, honouring sensor orientation.
///
/// `raw` is the average-mode u16 slot (width*height). `matrix` is the camera->output 3x3 (row-major) applied in linear light before sqrt. `gain` mirrors the display gain. Returns (out_w, out_h, rgb8) already rotated to upright.
pub fn debayer_to_rgb8(
    raw: &[u16],
    width: usize,
    height: usize,
    black_level: u16,
    bayer_pattern: u32,
    matrix: &[f32; 9],
    orientation: u16,
) -> (usize, usize, Vec<u8>) {
    // Output dimensions swap for 90/270.
    let (out_w, out_h) = match orientation {
        90 | 270 => (height, width),
        _ => (width, height),
    };
    let mut rgb = vec![0u8; out_w * out_h * 3];
    let scale = 65536. / (65536. - black_level as f32);

    for oy in 0..out_h {
        for ox in 0..out_w {
            // Map upright output coords back to sensor coords.
            let (sx, sy) = match orientation {
                90 => (oy, out_w - 1 - ox),
                180 => (width - 1 - ox, height - 1 - oy),
                270 => (height - 1 - oy, ox),
                _ => (ox, oy),
            };
            if sx >= width || sy >= height {
                continue;
            }
            // 2x2 bayer block at (sx,sy), same scheme as the preview.
            let bx = sx & !1;
            let by = sy & !1;
            let idx = by * width + bx;
            let tl = raw[idx] as f32;
            // MEMORY SAFETY: clamp the 2x2 Bayer-block neighbour index to the last valid array index (width*height-1) so reading the block at the right/bottom edge (where idx+1 / idx+width / idx+width+1 can exceed the buffer) can't index out of bounds and panic.
            let tr = raw[(idx + 1).min(width * height - 1)] as f32;
            // same as above: clamp the bottom-left neighbour index to the last valid index so a bottom-edge block can't read past the buffer.
            let bl = raw[(idx + width).min(width * height - 1)] as f32;
            // same as above: clamp the bottom-right neighbour index to the last valid index so a bottom-right-corner block can't read past the buffer.
            let br = raw[(idx + width + 1).min(width * height - 1)] as f32;
            let local = (sx - bx) + (sy - by);
            let (mut r, mut g, mut b) = match bayer_pattern {
                0 => (tl, if local < 2 { tr } else { bl }, br), // RGGB
                1 => (tr, if local < 2 { tl } else { br }, bl), // GRBG
                2 => (bl, if local < 2 { tl } else { br }, tr), // GBRG
                3 => (br, if local < 2 { tr } else { bl }, tl), // BGGR
                _ => (tl, if local < 2 { tr } else { bl }, br),
            };
            // Black subtract + gain in linear, then colour matrix, then sqrt encode. No floor: a sub-black value is real signal that stays f32 through the matrix; the final sqrt + saturating cast handle any negative output. Clamping here would discard noise-floor info and could brighten another channel via the matrix's negative coeffs.
            r = (r - black_level as f32) * scale;
            g = (g - black_level as f32) * scale;
            b = (b - black_level as f32) * scale;
            let lr = matrix[0] * r + matrix[1] * g + matrix[2] * b;
            let lg = matrix[3] * r + matrix[4] * g + matrix[5] * b;
            let lb = matrix[6] * r + matrix[7] * g + matrix[8] * b;
            let o = (oy * out_w + ox) * 3;
            // sqrt then cast to u8. No clamps needed: `as u8` is a saturating cast, so >255 -> 255, and a negative matrix result -> sqrt = NaN -> 0 (black) -- exactly what an explicit max(0)/min(255) would produce. Verified equivalent over all f32.
            rgb[o] = lr.sqrt() as u8;
            rgb[o + 1] = lg.sqrt() as u8;
            rgb[o + 2] = lb.sqrt() as u8;
        }
    }
    (out_w, out_h, rgb)
}

/// RCD-demosaic the full frame, colour-correct, and sqrt-encode to 8-bit RGB, honouring sensor orientation. Same interface as [debayer_to_rgb8] but with the higher-quality RCD demosaic (used for saved RGB exports — the 2x2 block path stays for the live preview's speed).
pub fn rcd_to_rgb8(
    raw: &[u16],
    width: usize,
    height: usize,
    black_level: u16,
    bayer_pattern: u32,
    matrix: &[f32; 9],
    orientation: u16,
) -> (usize, usize, Vec<u8>) {
    use crate::debayer::region::rcd_region;

    let scale = 65536. / (65536. - black_level as f32);
    // Demosaic the whole frame in one shot (crop == full frame). RCD returns black-subtracted + gained linear-ish RGB per sensor pixel.
    let demosaiced = rcd_region(
        raw,
        width,
        height,
        0,
        0,
        width,
        height,
        black_level,
        scale,
        bayer_pattern,
    );

    let (out_w, out_h) = match orientation {
        90 | 270 => (height, width),
        _ => (width, height),
    };
    let mut rgb = vec![0u8; out_w * out_h * 3];
    for oy in 0..out_h {
        for ox in 0..out_w {
            let (sx, sy) = match orientation {
                90 => (oy, out_w - 1 - ox),
                180 => (width - 1 - ox, height - 1 - oy),
                270 => (height - 1 - oy, ox),
                _ => (ox, oy),
            };
            if sx >= width || sy >= height {
                continue;
            }
            let px = demosaiced[sy * width + sx];
            let (r, g, b) = (px[0] as f32, px[1] as f32, px[2] as f32);
            let lr = matrix[0] * r + matrix[1] * g + matrix[2] * b;
            let lg = matrix[3] * r + matrix[4] * g + matrix[5] * b;
            let lb = matrix[6] * r + matrix[7] * g + matrix[8] * b;
            let o = (oy * out_w + ox) * 3;
            // sqrt then cast to u8. No clamps: `as u8` saturates (>255 -> 255), and a negative matrix result -> sqrt = NaN -> 0 (black), which is the desired output. Verified equivalent to max(0)/min(255) over all f32.
            rgb[o] = lr.sqrt() as u8;
            rgb[o + 1] = lg.sqrt() as u8;
            rgb[o + 2] = lb.sqrt() as u8;
        }
    }
    (out_w, out_h, rgb)
}

/// Quad-Bayer (Tetracell, max-res 50MP) demosaic to colour-corrected sqrt-encoded 8-bit RGB, honouring orientation. Same interface as [rcd_to_rgb8] but for the 4x4 quad-Bayer CFA. See [crate::debayer::quad].
pub fn quad_to_rgb8(
    raw: &[u16],
    width: usize,
    height: usize,
    black_level: u16,
    bayer_pattern: u32,
    matrix: &[f32; 9],
    orientation: u16,
) -> (usize, usize, Vec<u8>) {
    use crate::debayer::quad::quad_demosaic;

    // Match rcd_to_rgb8's scale: it feeds the colour matrix values in 0..~65535 via
    // (v-black) * 65536/(65536-black). We get the identical scale with white=65536 and gain=65536.
    let demosaiced = quad_demosaic(
        raw,
        width,
        height,
        black_level,
        65535,
        65536.0,
        bayer_pattern,
    );

    let (out_w, out_h) = match orientation {
        90 | 270 => (height, width),
        _ => (width, height),
    };
    let mut rgb = vec![0u8; out_w * out_h * 3];
    for oy in 0..out_h {
        for ox in 0..out_w {
            let (sx, sy) = match orientation {
                90 => (oy, out_w - 1 - ox),
                180 => (width - 1 - ox, height - 1 - oy),
                270 => (height - 1 - oy, ox),
                _ => (ox, oy),
            };
            if sx >= width || sy >= height {
                continue;
            }
            let px = demosaiced[sy * width + sx];
            let (r, g, b) = (px[0], px[1], px[2]);
            let lr = matrix[0] * r + matrix[1] * g + matrix[2] * b;
            let lg = matrix[3] * r + matrix[4] * g + matrix[5] * b;
            let lb = matrix[6] * r + matrix[7] * g + matrix[8] * b;
            let o = (oy * out_w + ox) * 3;
            // sqrt then cast to u8. No clamps: `as u8` saturates (>255 -> 255), and a negative matrix result -> sqrt = NaN -> 0 (black), which is the desired output. Verified equivalent to max(0)/min(255) over all f32.
            rgb[o] = lr.sqrt() as u8;
            rgb[o + 1] = lg.sqrt() as u8;
            rgb[o + 2] = lb.sqrt() as u8;
        }
    }
    (out_w, out_h, rgb)
}

/// Encode RGB8 to JPEG bytes (quality 95), tagged with the Rec.2020 ICC profile (APP2 marker).
pub fn encode_jpeg(rgb: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    use crate::image::icc::{jpeg_with_icc, rec2020_icc};
    let mut out = Cursor::new(Vec::new());
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95);
    enc.encode(rgb, width, height, image::ColorType::Rgb8)
        .ok()?;
    Some(jpeg_with_icc(&out.into_inner(), rec2020_icc()))
}

/// Encode RGB8 to lossless Deflate-compressed TIFF bytes.
///
/// The `image` crate's TIFF encoder is uncompressed-only, so we use the `tiff` crate directly to get lossless Deflate (≈1.5-2x smaller than raw RGB, still universally readable).
pub fn encode_tiff(rgb: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    use crate::image::icc::rec2020_icc;
    use tiff::encoder::{colortype::RGB8, compression::Deflate, TiffEncoder};
    use tiff::tags::Tag;
    let mut out = Cursor::new(Vec::new());
    {
        let mut enc = TiffEncoder::new(&mut out).ok()?;
        // Use the lower-level image encoder so we can write the ICCProfile tag (0x8773 = 34675) alongside the pixel data, keeping lossless Deflate.
        let mut image = enc
            .new_image_with_compression::<RGB8, _>(width, height, Deflate::default())
            .ok()?;
        image
            .encoder()
            .write_tag(Tag::Unknown(34675), rec2020_icc())
            .ok()?;
        image.write_data(rgb).ok()?;
    }
    Some(out.into_inner())
}

/// Encode RGB8 to JPEG XL bytes (lossless, via zune-jpegxl).
pub fn encode_jpegxl(rgb: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    use zune_core::bit_depth::BitDepth;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::EncoderOptions;
    use zune_jpegxl::JxlSimpleEncoder;

    let opts = EncoderOptions::new(width, height, ColorSpace::RGB, BitDepth::Eight);
    // Lumis fork of zune-jpegxl: signal Rec.2020 in the codestream's ColourEncoding so the wide-gamut output is interpreted correctly (upstream hardcodes sRGB).
    let encoder = JxlSimpleEncoder::new(rgb, opts).set_rec2020(true);
    let mut out: Vec<u8> = Vec::new();
    encoder.encode(&mut out).ok()?;
    Some(out)
}
