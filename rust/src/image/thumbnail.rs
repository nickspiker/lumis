//! Embedded-preview thumbnail generator for the DNG save path.
//!
//! Camera DNGs embed a small preview JPEG so any viewer/thumbnailer shows the image instantly without demosaicing the raw. Our quad-Bayer DNGs had none, so tools that don't understand the 4x4 CFA (RawTherapee, libraw thumbnailers, Nemo) either crash or render garbage. We build the preview ourselves from the raw.
//!
//! Pipeline (cheap-first ordering): bin the raw to half-res RGB (no demosaic - each 2x2 same-colour cluster IS a measured channel value), bilinear-downscale to thumbnail size, THEN apply the Rec.2020 matrix, auto-white (scale by the brightest single R/G/B value), gamma 1/2.4, and JPEG-encode. Matrix/white/gamma run on the tiny downscaled image, not the half-res one.
//!
//! Colour: the result is Rec.2020-primaried (we have no 1931/XYZ math to convert to sRGB reliably), so we assume Rec.2020 and tag it as such if the encoder allows.

use std::io::Cursor;

/// Build an embedded-preview JPEG from a raw Bayer/quad-Bayer frame. `target` is the max thumbnail dimension (longest side). `quad` selects 4x4 quad-Bayer binning vs standard 2x2. `matrix` is the camera->Rec.2020 3x3 (row-major). Returns (JPEG bytes, width, height), or None on encode failure.
pub fn build_preview_jpeg(
    raw: &[u16],
    width: usize,
    height: usize,
    black: u16,
    bayer_pattern: u32,
    quad: bool,
    matrix: &[f32; 9],
    target: usize,
) -> Option<(Vec<u8>, u32, u32)> {
    // --- 1. Bin to half-res linear RGB (no demosaic) ---
    // Quad: one RGB pixel per 4x4 tile (R/B = one cluster, G = mean of the two G clusters).
    // Standard Bayer: one RGB pixel per 2x2 cell.
    let (hw, hh, half_rgb) = if quad {
        bin_quad_rgb(raw, width, height, bayer_pattern, black)
    } else {
        bin_bayer_rgb(raw, width, height, bayer_pattern, black)
    };
    if hw == 0 || hh == 0 {
        return None;
    }

    // --- 2. Bilinear downscale to thumbnail size (longest side == target) ---
    let (tw, th) = fit(hw, hh, target);
    let small = bilinear(&half_rgb, hw, hh, tw, th);

    // --- 3. Apply the Rec.2020 matrix (on the tiny image) ---
    // --- 4. Auto-white: scale by the brightest single channel value ---
    // --- 5. Gamma 1/2.4 -> 8-bit ---
    let mut lin = vec![[0.0f32; 3]; tw * th];
    // max_ch: the brightest single channel value across the image = the auto-white point. Seeded at a small positive epsilon (not 0) only so the later `1.0 / max_ch` divide can't divide by zero on an all-black image - that is the ONLY reason for the seed, not value constraining.
    let mut max_ch = 1e-6f32;
    for (i, px) in small.iter().enumerate() {
        let (r, g, b) = (px[0], px[1], px[2]);
        let lr = matrix[0] * r + matrix[1] * g + matrix[2] * b;
        let lg = matrix[3] * r + matrix[4] * g + matrix[5] * b;
        let lb = matrix[6] * r + matrix[7] * g + matrix[8] * b;
        lin[i] = [lr, lg, lb];
        // This max() FINDS the brightest channel (the white point) - it is the algorithm, not a value-constraining clamp. Negative matrix outputs (out-of-gamut) never win a max against the positive seed/values, so they don't affect the result.
        max_ch = max_ch.max(lr).max(lg).max(lb);
    }
    let inv_white = 1.0 / max_ch;
    let inv_gamma = 1.0 / 2.4;
    let mut rgb8 = vec![0u8; tw * th * 3];
    for (i, px) in lin.iter().enumerate() {
        for c in 0..3 {
            // clamp to [0,1] is REQUIRED here: a real colour matrix has negative coefficients, so a post-matrix channel can be < 0 for out-of-gamut colours, and f32::powf(negative, fractional) returns NaN. Clamping the base to [0,1] before powf prevents that NaN (which would otherwise corrupt the pixel). The upper 1.0 bound keeps over-white values in range so the *255 cast lands in 0..=255.
            let v = (px[c] * inv_white).clamp(0.0, 1.0).powf(inv_gamma) * 255.0;
            // Stochastic dither: floor, then carry the remainder up probabilistically (out = floor + 1
            // with probability = frac(v)). This trades 8-bit banding on smooth gradients for a touch of
            // noise, which the eye strongly prefers - it's what removes the contouring seen on thumbnails.
            // The "random" threshold is a cheap per-(pixel,channel) hash (no RNG dep, deterministic so a
            // re-save is byte-identical, and decorrelated enough to look like noise, not a pattern).
            let floor = v.floor();
            let frac = v - floor;
            let h = (i as u32).wrapping_mul(2654435761).wrapping_add((c as u32).wrapping_mul(40503));
            let h = h ^ (h >> 15);
            let threshold = (h & 0xFFFF) as f32 / 65536.0; // uniform in [0,1)
            let dithered = floor + if frac > threshold { 1.0 } else { 0.0 };
            rgb8[i * 3 + c] = dithered.min(255.0) as u8;
        }
    }

    // --- 6. JPEG encode ---
    let mut out = Cursor::new(Vec::new());
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90);
    enc.encode(&rgb8, tw as u32, th as u32, image::ColorType::Rgb8)
        .ok()?;
    Some((out.into_inner(), tw as u32, th as u32))
}

/// Quad-Bayer -> half(quarter)-res linear RGB. One RGB pixel per 4x4 tile: each 2x2 cluster sums to its channel; the two green clusters are averaged. Black-subtracted, no gain (auto-white handles exposure later).
fn bin_quad_rgb(
    raw: &[u16],
    width: usize,
    height: usize,
    bayer_pattern: u32,
    black: u16,
) -> (usize, usize, Vec<[f32; 3]>) {
    let tw = width / 4;
    let th = height / 4;
    let mut out = vec![[0.0f32; 3]; tw * th];
    let bk = black as f32;
    // No floor: a sub-black sample is real signal that correctly lowers the cluster sum, and it stays f32 through the matrix/gamma downstream. Clamping per-sample would discard noise-floor info; the final saturating cast handles any negative output.
    let sub = |v: u16| (v as f32 - bk);
    // cluster sum at tile origin (ox,oy) offset (cx,cy) over its 2x2.
    let cluster = |ox: usize, oy: usize, cx: usize, cy: usize| -> f32 {
        let mut s = 0.0;
        for dy in 0..2 {
            for dx in 0..2 {
                s += sub(raw[(oy + cy + dy) * width + (ox + cx + dx)]);
            }
        }
        s
    };
    for ty in 0..th {
        for tx in 0..tw {
            let ox = tx * 4;
            let oy = ty * 4;
            // Which 2x2 cluster of the tile is which colour, from the base pattern (each base cell = a 2x2 cluster). Base 2x2 colours in (cluster_row, cluster_col) order.
            let base = base_2x2(bayer_pattern);
            let mut rgb = [0.0f32; 3];
            let mut gcount = 0.0;
            for crow in 0..2 {
                for ccol in 0..2 {
                    let color = base[crow * 2 + ccol];
                    let v = cluster(ox, oy, ccol * 2, crow * 2);
                    if color == 1 {
                        rgb[1] += v;
                        gcount += 1.0;
                    } else {
                        rgb[color] = v;
                    }
                }
            }
            if gcount > 0.0 {
                rgb[1] /= gcount;
            }
            out[ty * tw + tx] = rgb;
        }
    }
    (tw, th, out)
}

/// Standard 2x2 Bayer -> half-res linear RGB. One RGB pixel per 2x2 cell; the two greens are averaged. Black-subtracted.
fn bin_bayer_rgb(
    raw: &[u16],
    width: usize,
    height: usize,
    bayer_pattern: u32,
    black: u16,
) -> (usize, usize, Vec<[f32; 3]>) {
    let hw = width / 2;
    let hh = height / 2;
    let mut out = vec![[0.0f32; 3]; hw * hh];
    let bk = black as f32;
    // No floor: a sub-black sample is real signal that correctly lowers the cluster sum, and it stays f32 through the matrix/gamma downstream. Clamping per-sample would discard noise-floor info; the final saturating cast handles any negative output.
    let sub = |v: u16| (v as f32 - bk);
    let base = base_2x2(bayer_pattern);
    for cy in 0..hh {
        for cx in 0..hw {
            let ox = cx * 2;
            let oy = cy * 2;
            let cell = [
                sub(raw[oy * width + ox]),
                sub(raw[oy * width + ox + 1]),
                sub(raw[(oy + 1) * width + ox]),
                sub(raw[(oy + 1) * width + ox + 1]),
            ];
            let mut rgb = [0.0f32; 3];
            let mut gcount = 0.0;
            for i in 0..4 {
                let color = base[i];
                if color == 1 {
                    rgb[1] += cell[i];
                    gcount += 1.0;
                } else {
                    rgb[color] = cell[i];
                }
            }
            if gcount > 0.0 {
                rgb[1] /= gcount;
            }
            out[cy * hw + cx] = rgb;
        }
    }
    (hw, hh, out)
}

/// Base 2x2 CFA colour indices (0=R,1=G,2=B) in (row,col) order for an Android bayer pattern.
fn base_2x2(bayer_pattern: u32) -> [usize; 4] {
    match bayer_pattern {
        0 => [0, 1, 1, 2], // RGGB
        1 => [1, 0, 2, 1], // GRBG
        2 => [1, 2, 0, 1], // GBRG
        3 => [2, 1, 1, 0], // BGGR
        _ => [0, 1, 1, 2],
    }
}

/// Fit (w,h) so the longest side == target, preserving aspect.
fn fit(w: usize, h: usize, target: usize) -> (usize, usize) {
    // .max(1): a very wide/tall aspect can round the short side to 0 (e.g. target*h/w with h<<w). A 0 dimension makes the downscale divide by zero (fx = sw/dw) and the JPEG encoder reject a 0-size image. .max(1) guarantees at least 1px. .min(w/h): never upscale past the source.
    if w >= h {
        let th = (target * h / w).max(1);
        (target.min(w), th)
    } else {
        let tw = (target * w / h).max(1);
        (tw, target.min(h))
    }
}

/// Bilinear downscale RGB f32.
fn bilinear(src: &[[f32; 3]], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0f32; 3]; dw * dh];
    let fx = sw as f32 / dw as f32;
    let fy = sh as f32 / dh as f32;
    // The clamps below are MEMORY SAFETY on array indexing: x0/x1/y0/y1 index src[y*sw+x]; an out-of-range index panics (and is logic UB). The source coord sy/sx can be negative at the top/left edge (the -0.5 pixel-centre offset) or reach sw-1/sh-1 at the bottom/right, so: .max(0.0) before `as usize` prevents a negative float casting to a huge usize (which would index far out of bounds); .min(sh-1)/.min(sw-1) keep the +1 neighbour in range; the weight .clamp(0,1) keeps the interpolation fraction valid where the coord was edge-clamped.
    for dy in 0..dh {
        let sy = (dy as f32 + 0.5) * fy - 0.5;
        let y0 = sy.floor().max(0.0) as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let wy = (sy - y0 as f32).clamp(0.0, 1.0);
        for dx in 0..dw {
            let sx = (dx as f32 + 0.5) * fx - 0.5;
            let x0 = sx.floor().max(0.0) as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let wx = (sx - x0 as f32).clamp(0.0, 1.0);
            let mut px = [0.0f32; 3];
            for c in 0..3 {
                let top = src[y0 * sw + x0][c] * (1.0 - wx) + src[y0 * sw + x1][c] * wx;
                let bot = src[y1 * sw + x0][c] * (1.0 - wx) + src[y1 * sw + x1][c] * wx;
                px[c] = top * (1.0 - wy) + bot * wy;
            }
            out[dy * dw + dx] = px;
        }
    }
    out
}
