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
    gain: f32,
) -> (usize, usize, Vec<u8>) {
    use crate::debayer::region::rcd_region;

    // gain bakes the user's display-gain slider into the exported pixels so the file matches the preview.
    let scale = gain * 65536. / (65536. - black_level as f32);
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
    gain: f32,
) -> (usize, usize, Vec<u8>) {
    use crate::debayer::quad::quad_demosaic;

    // Match rcd_to_rgb8's scale: it feeds the colour matrix values in 0..~65535 via
    // (v-black) * 65536/(65536-black). We get the identical scale with white=65536 and gain=65536, times
    // the user's display-gain slider so the export matches the on-screen brightness.
    let demosaiced = quad_demosaic(
        raw,
        width,
        height,
        black_level,
        65535,
        65536.0 * gain,
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
pub fn encode_jpeg(
    rgb: &[u8],
    width: u32,
    height: u32,
    description: &str,
    exif: &crate::image::dng::ExifData,
) -> Option<Vec<u8>> {
    use crate::image::icc::{jpeg_with_icc, rec2020_icc};
    let mut out = Cursor::new(Vec::new());
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95);
    enc.encode(rgb, width, height, image::ColorType::Rgb8)
        .ok()?;
    let with_icc = jpeg_with_icc(&out.into_inner(), rec2020_icc());
    let with_exif = jpeg_with_exif(&with_icc, exif);
    Some(jpeg_with_comment(&with_exif, description))
}

/// Insert a JPEG APP1 EXIF segment (marker 0xFFE1, "Exif\0\0" + a standalone TIFF/EXIF block) after SOI,
/// so viewers show native ExposureTime/FNumber/ISO/FocalLength/DateTime fields. No-op if there's no EXIF.
fn jpeg_with_exif(jpeg: &[u8], exif: &crate::image::dng::ExifData) -> Vec<u8> {
    let block = crate::image::dng::build_exif_block(exif);
    if block.is_empty() || jpeg.len() < 2 {
        return jpeg.to_vec();
    }
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&block);
    // Marker segment length covers the 2 length bytes + payload; APP1 caps at 65535.
    if payload.len() + 2 > 65535 {
        return jpeg.to_vec();
    }
    let seg_len = (payload.len() + 2) as u16;
    let mut out = Vec::with_capacity(jpeg.len() + payload.len() + 4);
    out.extend_from_slice(&jpeg[0..2]); // SOI
    out.extend_from_slice(&[0xFF, 0xE1]); // APP1
    out.extend_from_slice(&seg_len.to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&jpeg[2..]); // rest of the JPEG
    out
}

/// Insert a JPEG COM (comment) marker carrying `text` right after SOI. exiftool/most tools surface this
/// as the image Comment. Used to carry the exposure summary (per-frame + composite), matching DNG/TIFF.
/// Empty text returns the input unchanged. The COM payload is capped at the 65533-byte marker limit.
fn jpeg_with_comment(jpeg: &[u8], text: &str) -> Vec<u8> {
    if text.is_empty() || jpeg.len() < 2 {
        return jpeg.to_vec();
    }
    let bytes = text.as_bytes();
    let payload = &bytes[..bytes.len().min(65533)];
    let seg_len = (payload.len() + 2) as u16; // length field includes its own 2 bytes
    let mut out = Vec::with_capacity(jpeg.len() + payload.len() + 4);
    out.extend_from_slice(&jpeg[0..2]); // SOI
    out.extend_from_slice(&[0xFF, 0xFE]); // COM marker
    out.extend_from_slice(&seg_len.to_be_bytes());
    out.extend_from_slice(payload);
    out.extend_from_slice(&jpeg[2..]); // rest of the JPEG
    out
}

/// Encode RGB8 to lossless Deflate-compressed TIFF bytes, with a small JPEG thumbnail embedded in a chained IFD1.
///
/// The `image` crate's TIFF encoder is uncompressed-only, so we use the `tiff` crate directly to get lossless Deflate (≈1.5-2x smaller than raw RGB, still universally readable).
///
/// Big 50MP TIFFs fail to thumbnail in Android file managers/galleries (ExifInterface logs "No image meets the size requirements of a thumbnail image"). The `tiff` crate writes IFD0 with a next-IFD pointer of 0 and does not expose that pointer, so we post-process its output: append a thumbnail JPEG and a chained IFD1 (classic TIFF thumbnail convention - exiftool/galleries read IFD1 of a TIFF as the thumbnail), then patch IFD0's next-IFD u32 to point at IFD1. If thumbnail generation fails we still return the valid TIFF without a thumbnail rather than failing the whole save.
pub fn encode_tiff(
    rgb: &[u8],
    width: u32,
    height: u32,
    description: &str,
    exif: &crate::image::dng::ExifData,
) -> Option<Vec<u8>> {
    use crate::image::icc::rec2020_icc;
    use tiff::encoder::{colortype::RGB8, compression::Deflate, Rational, TiffEncoder};
    use tiff::tags::Tag;
    // EXIF as a RATIONAL (num/den, 1000 denom preserving sub-1.0 values).
    let rat = |v: f64| -> Rational {
        if v >= 1.0 {
            Rational { n: (v * 1000.0).round() as u32, d: 1000 }
        } else {
            Rational { n: 1000, d: (1000.0 / v).round() as u32 }
        }
    };
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
        // ImageDescription (tag 270): the exposure summary (per-frame + composite/effective), matching
        // the DNG. Skip when empty so we don't write a stray empty tag.
        if !description.is_empty() {
            image
                .encoder()
                .write_tag(Tag::Unknown(270), description)
                .ok()?;
        }
        // EXIF tags written directly into IFD0 (viewers read ExposureTime/FNumber/etc. there too). The
        // tiff crate's sub-IFD support is awkward, and IFD0 placement is universally understood. Tags in
        // ascending id order. RATIONAL via the crate's Rational; ISO/35mm-equiv as SHORT.
        let e = exif;
        if e.exposure_time_s > 0.0 {
            image.encoder().write_tag(Tag::Unknown(0x829A), rat(e.exposure_time_s)).ok()?;
        }
        if e.f_number > 0.0 {
            image.encoder().write_tag(Tag::Unknown(0x829D), rat(e.f_number)).ok()?;
        }
        if e.iso > 0.0 {
            image.encoder().write_tag(Tag::Unknown(0x8827), (e.iso.round() as u32).min(65535) as u16).ok()?;
        }
        if !e.datetime_original.is_empty() {
            image.encoder().write_tag(Tag::Unknown(0x9003), e.datetime_original.as_str()).ok()?;
        }
        if e.subject_distance_m > 0.0 {
            image.encoder().write_tag(Tag::Unknown(0x9206), rat(e.subject_distance_m)).ok()?;
        }
        if e.focal_length_mm > 0.0 {
            image.encoder().write_tag(Tag::Unknown(0x920A), rat(e.focal_length_mm)).ok()?;
        }
        if e.focal_length_35mm > 0.0 {
            image.encoder().write_tag(Tag::Unknown(0xA405), (e.focal_length_35mm.round() as u32).min(65535) as u16).ok()?;
        }
        // GPS IFD pointer (0x8825) as a placeholder 0 - the GPS sub-IFD is appended + the offset patched in
        // post-process (the tiff crate can't write a sub-IFD itself). Only when there's a fix.
        if e.has_gps {
            image.encoder().write_tag(Tag::Unknown(0x8825), 0u32).ok()?;
        }
        image.write_data(rgb).ok()?;
    }
    let mut tiff = out.into_inner();
    // Embed the GPS sub-IFD (the crate wrote a placeholder 0x8825 pointer = 0; we append the IFD and patch
    // it). Do this BEFORE the thumbnail (which chains IFD1 via the next-IFD pointer); GPS only appends data
    // + patches a value field, so order is independent, but keep GPS first for clarity. On any failure,
    // continue with the un-GPS'd TIFF.
    if exif.has_gps {
        if let Some(with_gps) = embed_tiff_gps(tiff.clone(), exif) {
            tiff = with_gps;
        }
    }
    // Try to embed a thumbnail; on any failure fall back to the plain (still valid) TIFF.
    match embed_tiff_thumbnail(tiff.clone(), rgb, width, height) {
        Some(with_thumb) => Some(with_thumb),
        None => Some(tiff),
    }
}

/// Post-process a TIFF the `tiff` crate produced (with a placeholder GPS-pointer tag 0x8825 = 0 in IFD0):
/// append the GPS sub-IFD at the file end and patch the pointer's value field to it. No byte insertion, so
/// existing absolute offsets (ICC, strips) stay valid. Returns None (caller keeps the original) on any
/// parse surprise.
fn embed_tiff_gps(mut tiff: Vec<u8>, exif: &crate::image::dng::ExifData) -> Option<Vec<u8>> {
    if tiff.len() < 8 || &tiff[0..2] != b"II" || u16::from_le_bytes([tiff[2], tiff[3]]) != 0x2A {
        return None;
    }
    let ifd0 = u32::from_le_bytes([tiff[4], tiff[5], tiff[6], tiff[7]]) as usize;
    if ifd0 + 2 > tiff.len() {
        return None;
    }
    let count = u16::from_le_bytes([tiff[ifd0], tiff[ifd0 + 1]]) as usize;
    // Find the 0x8825 entry and the position of its 4-byte value field.
    let mut gps_val_pos = None;
    for i in 0..count {
        let ep = ifd0 + 2 + i * 12;
        if ep + 12 > tiff.len() {
            return None;
        }
        if u16::from_le_bytes([tiff[ep], tiff[ep + 1]]) == 0x8825 {
            gps_val_pos = Some(ep + 8);
            break;
        }
    }
    let gps_val_pos = gps_val_pos?;
    // Build the GPS IFD bytes (count + entries + next-IFD=0 + out-of-line payloads) as a standalone block,
    // then place it at the current file end (absolute offsets, base = the append position).
    let gps_entries = crate::image::dng::build_gps_entries(exif);
    if gps_entries.is_empty() {
        return None;
    }
    if tiff.len() % 2 != 0 {
        tiff.push(0);
    }
    let gps_ifd_off = tiff.len() as u32;
    crate::image::dng::write_gps_ifd_into(&mut tiff, &gps_entries);
    tiff[gps_val_pos..gps_val_pos + 4].copy_from_slice(&gps_ifd_off.to_le_bytes());
    Some(tiff)
}

/// Downscale a u8 RGB image so the longest side is <= `target`, preserving aspect, and JPEG-encode it (quality 85). Returns (JPEG bytes, thumb_width, thumb_height) or None on failure.
fn build_tiff_thumbnail(
    rgb: &[u8],
    width: u32,
    height: u32,
    target: u32,
) -> Option<(Vec<u8>, u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    // Fit so the longest side == target, preserving aspect. .max(1): a very wide/tall aspect can round the short side to 0; a 0 dimension makes the downscale divide by zero and the JPEG encoder reject a 0-size image, so guarantee at least 1px. .min(w/h): never upscale past the source.
    let (tw, th) = if w >= h {
        let t = (target as usize).min(w);
        (t, (target as usize * h / w).max(1).min(h))
    } else {
        let t = (target as usize).min(h);
        ((target as usize * w / h).max(1).min(w), t)
    };
    // Bilinear downscale over u8 RGB. The clamps below are MEMORY SAFETY on array indexing: x0/x1/y0/y1 index rgb[(y*w+x)*3+c]; an out-of-range index panics. The source coord sx/sy can be negative at the top/left edge (the -0.5 pixel-centre offset) or reach w-1/h-1 at the bottom/right, so: .max(0.0) before `as usize` prevents a negative float casting to a huge usize (which would index far out of bounds); .min(w-1)/.min(h-1) keep the +1 neighbour in range; the weight .clamp(0,1) keeps the interpolation fraction valid where the coord was edge-clamped.
    let mut small = vec![0u8; tw * th * 3];
    let fx = w as f32 / tw as f32;
    let fy = h as f32 / th as f32;
    for dy in 0..th {
        let sy = (dy as f32 + 0.5) * fy - 0.5;
        let y0 = sy.floor().max(0.0) as usize;
        let y1 = (y0 + 1).min(h - 1);
        let wy = (sy - y0 as f32).clamp(0.0, 1.0);
        for dx in 0..tw {
            let sx = (dx as f32 + 0.5) * fx - 0.5;
            let x0 = sx.floor().max(0.0) as usize;
            let x1 = (x0 + 1).min(w - 1);
            let wx = (sx - x0 as f32).clamp(0.0, 1.0);
            for c in 0..3 {
                let p00 = rgb[(y0 * w + x0) * 3 + c] as f32;
                let p01 = rgb[(y0 * w + x1) * 3 + c] as f32;
                let p10 = rgb[(y1 * w + x0) * 3 + c] as f32;
                let p11 = rgb[(y1 * w + x1) * 3 + c] as f32;
                let top = p00 * (1.0 - wx) + p01 * wx;
                let bot = p10 * (1.0 - wx) + p11 * wx;
                let v = top * (1.0 - wy) + bot * wy;
                // The +0.5 rounds to nearest; the bilinear of in-range u8 values stays within [0,255] so no clamp is needed before the cast, but the `as u8` would wrap a stray out-of-range value - the inputs are u8 so v is provably in [0,255].
                small[(dy * tw + dx) * 3 + c] = (v + 0.5) as u8;
            }
        }
    }
    let mut out = Cursor::new(Vec::new());
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
    enc.encode(&small, tw as u32, th as u32, image::ColorType::Rgb8)
        .ok()?;
    Some((out.into_inner(), tw as u32, th as u32))
}

/// Post-process the crate's single-IFD TIFF: append a thumbnail JPEG and a chained IFD1, then patch IFD0's next-IFD pointer to it. Returns the augmented TIFF, or None if the thumbnail could not be generated or the header didn't match the expected little-endian classic-TIFF layout.
fn embed_tiff_thumbnail(
    mut tiff: Vec<u8>,
    rgb: &[u8],
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    // Thumbnail: max 512px longest side, JPEG quality 85.
    let (jpeg, tw, th) = build_tiff_thumbnail(rgb, width, height, 512)?;

    // Parse the classic-TIFF header: "II" (little-endian), magic 0x2A, then a u32 offset to IFD0. We only support the little-endian layout the `tiff` crate emits.
    if tiff.len() < 8 || &tiff[0..2] != b"II" {
        return None;
    }
    if u16::from_le_bytes([tiff[2], tiff[3]]) != 0x2A {
        return None;
    }
    let ifd0_offset = u32::from_le_bytes([tiff[4], tiff[5], tiff[6], tiff[7]]) as usize;
    if ifd0_offset + 2 > tiff.len() {
        return None;
    }
    let count = u16::from_le_bytes([tiff[ifd0_offset], tiff[ifd0_offset + 1]]) as usize;
    // The next-IFD pointer is the u32 at ifd0_offset + 2 + count*12 (after the 2-byte entry count and each 12-byte entry).
    let next_ifd_ptr_pos = ifd0_offset + 2 + count * 12;
    if next_ifd_ptr_pos + 4 > tiff.len() {
        return None;
    }
    // Confirm the crate wrote 0 there (single-IFD file). If not, the layout isn't what we expect and we must not corrupt it.
    let existing = u32::from_le_bytes([
        tiff[next_ifd_ptr_pos],
        tiff[next_ifd_ptr_pos + 1],
        tiff[next_ifd_ptr_pos + 2],
        tiff[next_ifd_ptr_pos + 3],
    ]);
    if existing != 0 {
        return None;
    }

    let word = 2usize; // word-align appended blocks to 2 bytes (classic TIFF alignment).
    // Append the thumbnail JPEG, word-aligned.
    if tiff.len() % word != 0 {
        tiff.extend(vec![0u8; word - tiff.len() % word]);
    }
    let jpeg_offset = tiff.len() as u32;
    tiff.extend(&jpeg);
    if tiff.len() % word != 0 {
        tiff.extend(vec![0u8; word - tiff.len() % word]);
    }

    // The chained IFD1 (thumbnail) starts here.
    let ifd1_offset = tiff.len() as u32;
    // TIFF tags (LE): tag(2) type(2) count(4) value/offset(4). Types: 3=SHORT, 4=LONG.
    let entries: [(u16, u16, u32, u32); 8] = [
        (254, 4, 1, 1),                 // NewSubfileType = 1 (reduced-resolution / thumbnail)
        (256, 4, 1, tw),                // ImageWidth
        (257, 4, 1, th),                // ImageLength
        (258, 3, 1, 8),                 // BitsPerSample = 8
        (259, 3, 1, 7),                 // Compression = 7 (JPEG)
        (262, 3, 1, 6),                 // PhotometricInterpretation = 6 (YCbCr, JPEG)
        (513, 4, 1, jpeg_offset),       // JPEGInterchangeFormat (offset to JPEG)
        (514, 4, 1, jpeg.len() as u32), // JPEGInterchangeFormatLength
    ];
    tiff.extend((entries.len() as u16).to_le_bytes());
    for (tag, typ, count, val) in entries {
        tiff.extend(tag.to_le_bytes());
        tiff.extend(typ.to_le_bytes());
        tiff.extend(count.to_le_bytes());
        // For SHORT (type 3) the value sits in the low 2 bytes of the 4-byte field, LE.
        tiff.extend(val.to_le_bytes());
    }
    tiff.extend([0u8, 0, 0, 0]); // IFD1 next-IFD pointer = 0 (end of chain)

    // Patch IFD0's next-IFD pointer to point at IFD1.
    tiff[next_ifd_ptr_pos..next_ifd_ptr_pos + 4].copy_from_slice(&ifd1_offset.to_le_bytes());

    Some(tiff)
}

/// Encode RGB8 to JPEG XL bytes (lossless, via zune-jpegxl).
pub fn encode_jpegxl(
    rgb: &[u8],
    width: usize,
    height: usize,
    exif: &crate::image::dng::ExifData,
) -> Option<Vec<u8>> {
    use zune_core::bit_depth::BitDepth;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::EncoderOptions;
    use zune_jpegxl::JxlSimpleEncoder;

    let opts = EncoderOptions::new(width, height, ColorSpace::RGB, BitDepth::Eight);
    // Lumis fork of zune-jpegxl: signal Rec.2020 in the codestream's ColourEncoding so the wide-gamut output is interpreted correctly (upstream hardcodes sRGB).
    let encoder = JxlSimpleEncoder::new(rgb, opts).set_rec2020(true);
    let mut codestream: Vec<u8> = Vec::new();
    encoder.encode(&mut codestream).ok()?;

    // The encoder emits a RAW JXL codestream (starts FF 0A), which can't hold metadata. To embed EXIF we
    // wrap it in the ISOBMFF box container. If there's no EXIF, return the raw codestream unchanged.
    let exif_block = crate::image::dng::build_exif_block(exif);
    if exif_block.is_empty() {
        return Some(codestream);
    }
    let mut out: Vec<u8> = Vec::new();
    // Append one ISOBMFF box: 4-byte big-endian size (incl header) + 4-byte type + payload.
    let mut push_box = |out: &mut Vec<u8>, btype: &[u8; 4], payload: &[u8]| {
        let size = (8 + payload.len()) as u32;
        out.extend(size.to_be_bytes());
        out.extend_from_slice(btype);
        out.extend_from_slice(payload);
    };
    // JXL signature box (special 12-byte form) + ftyp.
    out.extend([0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A]);
    push_box(&mut out, b"ftyp", &[b'j', b'x', b'l', b' ', 0, 0, 0, 0, b'j', b'x', b'l', b' ']);
    // Exif box: 4-byte big-endian TIFF-header offset (0 = TIFF follows immediately) + the EXIF/TIFF block.
    let mut exif_payload = vec![0u8, 0, 0, 0];
    exif_payload.extend_from_slice(&exif_block);
    push_box(&mut out, b"Exif", &exif_payload);
    // jxlc box: the raw codestream verbatim.
    push_box(&mut out, b"jxlc", &codestream);
    Some(out)
}
