//! Region-scoped RCD demosaic shared by the save pipeline and the zoomed-in preview.
//!
//! RCD ([crate::debayer::rcd]) is a 5x5-neighbourhood algorithm: it skips a 4px border and reads +/-4 rows/cols, so demosaicing a sub-rectangle of the sensor requires feeding it the visible crop PLUS a margin, then discarding that margin. We also snap the padded origin to an even (row,col) so the crop keeps the sensor's Bayer phase — otherwise the `fc` colour lookup is shifted and channels swap.
//!
//! The save path passes the whole frame as the "crop"; the preview passes only the visible window. Output is linear-ish u16 RGB (the raw values RCD produced, per channel) for the requested region only — colour matrix / sqrt / orientation are the caller's job.

use crate::debayer::rcd::RcdData;
use crate::image::raw::RawImage;

/// Margin (px) added on every side before RCD so the visible region's edges land inside RCD's valid interior (it leaves a 4px border). Rounded out to an even count to preserve Bayer phase.
pub const RCD_MARGIN: usize = 8;

/// Map an Android Bayer pattern (0=RGGB,1=GRBG,2=GBRG,3=BGGR) to an `fc(row,col)->channel` closure, where channel is 0=R,1=G,2=B. The pattern describes the top-left 2x2 of the sensor (even row, even col); `even`/`odd` of row+col select within the quad.
fn fc_for_pattern(pattern: u32) -> Box<dyn Fn(usize, usize) -> usize> {
    // The 2x2 quad in (row,col) order: [ (0,0),(0,1),(1,0),(1,1) ] as channel indices.
    let quad: [usize; 4] = match pattern {
        0 => [0, 1, 1, 2], // RGGB
        1 => [1, 0, 2, 1], // GRBG
        2 => [1, 2, 0, 1], // GBRG
        3 => [2, 1, 1, 0], // BGGR
        _ => [0, 1, 1, 2],
    };
    Box::new(move |row: usize, col: usize| quad[(row & 1) * 2 + (col & 1)])
}

/// Demosaic the rectangle [crop_x, crop_x+crop_w) x [crop_y, crop_y+crop_h) of a `full_w`x`full_h` Bayer frame with RCD, returning that exact region as row-major `[r,g,b]` u16 triples (crop_w*crop_h entries). `black`/`gain` are applied per CFA sample in linear before demosaic (gain mirrors the display's linear scale). The caller passes sensor-frame coords; the margin + phase alignment are handled internally.
pub fn rcd_region(
    raw: &[u16],
    full_w: usize,
    full_h: usize,
    crop_x: usize,
    crop_y: usize,
    crop_w: usize,
    crop_h: usize,
    black: u16,
    gain: f32,
    pattern: u32,
) -> Vec<[u16; 3]> {
    // Pad outwards by the margin, snap the origin DOWN to an even coord (Bayer phase), and clamp to the frame. Snapping to even keeps the padded tile's (0,0) on the same colour as the sensor origin so `fc_for_pattern` stays correct.
    // Rule 0: saturating_sub prevents usize underflow when crop_x < RCD_MARGIN (near-edge crop); it floors at 0 instead of wrapping to a huge usize that would later index out of bounds as a row/col origin.
    let pad_x0 = crop_x.saturating_sub(RCD_MARGIN) & !1;
    // Rule 0: saturating_sub prevents usize underflow when crop_y < RCD_MARGIN (near-edge crop); it floors at 0 instead of wrapping to a huge usize that would later index out of bounds as a row/col origin.
    let pad_y0 = crop_y.saturating_sub(RCD_MARGIN) & !1;
    // Rule 0 MEMORY SAFETY: .min(full_w) clamps the padded region's right edge to the frame width so later reads of this tile stay in bounds.
    let pad_x1 = (crop_x + crop_w + RCD_MARGIN).min(full_w);
    // Rule 0 MEMORY SAFETY: .min(full_h) clamps the padded region's bottom edge to the frame height so later reads of this tile stay in bounds.
    let pad_y1 = (crop_y + crop_h + RCD_MARGIN).min(full_h);
    let pad_w = pad_x1 - pad_x0;
    let pad_h = pad_y1 - pad_y0;

    // Load the padded tile into an RcdData grid, black-subtracting + gaining each CFA sample. fc uses padded-tile-local coords; because pad origin is even-aligned it matches the sensor.
    let scale = gain; // linear gain mirrors the preview's `scale`
    let fc = fc_for_pattern(pattern);
    let mut rcd = RcdData::new(pad_w, pad_h, fc_for_pattern(pattern));
    for ty in 0..pad_h {
        let sy = pad_y0 + ty;
        for tx in 0..pad_w {
            let sx = pad_x0 + tx;
            let v = raw[sy * full_w + sx];
            // RcdData stores u16, so the `as u16` here necessarily floors sub-black negatives to 0 (and saturates >65535). That loses noise-floor signal, but it's a constraint of RCD's u16 grid, not a removable clamp - preserving it would require RCD to work in f32. No explicit clamp added; the cast is the floor.
            let lin = ((v as f32 - black as f32) * scale) as u16;
            let ch = fc(ty, tx);
            rcd.data[ty][tx][ch] = lin;
        }
    }

    let off_x = crop_x - pad_x0;
    let off_y = crop_y - pad_y0;
    let mut out = vec![[0u16; 3]; crop_w * crop_h];

    // RCD's interior runs only over rows/cols 4..n-4 and needs n>8 to produce anything; below that the tile is all-border. For such degenerate tiles (tiny crops, or edge crops whose margin got clamped away) fall back to a cheap 2x2-block debayer so the region is never blank. This is the same nearest-block scheme as the live zoomed-out preview.
    if pad_w > 8 && pad_h > 8 {
        let raw_img = RawImage::new(pad_w, pad_h);
        rcd.rcd_demosaic(&raw_img);
        for y in 0..crop_h {
            for x in 0..crop_w {
                out[y * crop_w + x] = rcd.data[off_y + y][off_x + x];
            }
        }
    } else {
        for y in 0..crop_h {
            for x in 0..crop_w {
                let sx = crop_x + x;
                let sy = crop_y + y;
                out[y * crop_w + x] =
                    block_debayer_pixel(raw, full_w, full_h, sx, sy, black, gain, pattern);
            }
        }
    }
    out
}

/// Cheap 2x2-block "nearest" debayer of a single sensor pixel (black-subtracted + gained), matching the live preview's scheme. Used as the small-tile fallback for [rcd_region].
fn block_debayer_pixel(
    raw: &[u16],
    full_w: usize,
    full_h: usize,
    sx: usize,
    sy: usize,
    black: u16,
    gain: f32,
    pattern: u32,
) -> [u16; 3] {
    let bx = sx & !1;
    let by = sy & !1;
    let last = full_w * full_h - 1;
    // Rule 0 MEMORY SAFETY: .min(last) clamps the 2x2-block origin to the final valid index so reading the block at the image's right/bottom edge can't index out of bounds / panic.
    let idx = (by * full_w + bx).min(last);
    let tl = raw[idx] as f32;
    // Rule 0 MEMORY SAFETY: .min(last) clamps the right neighbour index to the final valid index so the edge block read can't index out of bounds / panic.
    let tr = raw[(idx + 1).min(last)] as f32;
    // Rule 0 MEMORY SAFETY: .min(last) clamps the below neighbour index to the final valid index so the edge block read can't index out of bounds / panic.
    let bl = raw[(idx + full_w).min(last)] as f32;
    // Rule 0 MEMORY SAFETY: .min(last) clamps the below-right neighbour index to the final valid index so the edge block read can't index out of bounds / panic.
    let br = raw[(idx + full_w + 1).min(last)] as f32;
    let local = (sx - bx) + (sy - by);
    let (r, g, b) = match pattern {
        0 => (tl, if local < 2 { tr } else { bl }, br), // RGGB
        1 => (tr, if local < 2 { tl } else { br }, bl), // GRBG
        2 => (bl, if local < 2 { tl } else { br }, tr), // GBRG
        3 => (br, if local < 2 { tr } else { bl }, tl), // BGGR
        _ => (tl, if local < 2 { tr } else { bl }, br),
    };
    // No clamps: each channel casts straight to u16 (no matrix between), so `as u16` saturates sub-black (negative) -> 0 and >65535 -> 65535.
    let cv = |v: f32| ((v - black as f32) * gain) as u16;
    [cv(r), cv(g), cv(b)]
}
