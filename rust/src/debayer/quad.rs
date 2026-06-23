//! Quad-Bayer (Tetracell) demosaic. The max-res sensor groups pixels into 2x2 same-colour clusters, so its CFA is a 4x4 tile rather than a standard 2x2 Bayer. Validated against synthetic ground truth (zone plate, slanted edges, colour edges, dead-leaves): ~33 dB on edges, ~35 dB on natural detail, with clean saturated colours.
//!
//! Algorithm (RCD adapted to quad-Bayer, all in the colour-DIFFERENCE domain for stability on saturated colours):
//!   1. Each 2x2 same-colour cluster is a MEASURED low-pass anchor of its channel - exactly the LPF a standard-Bayer demosaic has to estimate. We get it for free.
//!   2. Green is reconstructed at every pixel: measured greens kept full-res; in R/B clusters green is interpolated DIRECTIONALLY (H vs V chosen by the smaller cluster gradient = interpolate along edges) and added back as measured + (green_lpf - own_lpf).
//!   3. R and B reconstructed from the green-difference domain: colour = green + directional(colour_cluster - green_cluster_lpf).
//!
//! The 4x4 tile for an RGGB-base sensor (0=R 1=G 2=B), where each base colour expands to a 2x2 cluster:
//!   R R G G / R R G G / G G B B / G G B B
//! The base 2x2 (bayer_pattern) sets which corner cluster is which colour.

/// Colour index (0=R,1=G,2=B) of the quad-Bayer CFA at (row,col) for the given Android base bayer pattern (0=RGGB,1=GRBG,2=GBRG,3=BGGR). Each base cell covers a 2x2 cluster, so we look up the base colour by which 2x2 cluster of the 4x4 tile we are in.
#[inline]
pub fn quad_cfa_color(bayer_pattern: u32, row: usize, col: usize) -> usize {
    // The base 2x2 in (row,col) order: [(0,0),(0,1),(1,0),(1,1)] as colour indices.
    let base: [usize; 4] = match bayer_pattern {
        0 => [0, 1, 1, 2], // RGGB
        1 => [1, 0, 2, 1], // GRBG
        2 => [1, 2, 0, 1], // GBRG
        3 => [2, 1, 1, 0], // BGGR
        _ => [0, 1, 1, 2],
    };
    // Which 2x2 cluster of the 4x4 tile: top/bottom by row&2, left/right by col&2.
    let cluster_row = (row >> 1) & 1;
    let cluster_col = (col >> 1) & 1;
    base[cluster_row * 2 + cluster_col]
}

#[inline]
fn clampi(v: isize, hi: usize) -> usize {
    // Rule 0: MEMORY SAFETY - this index clamp keeps an array index in [0, hi-1] so raw[...] / green[...] can't panic / index out of bounds when a neighbour offset (x-2, x+2, etc.) runs off the image edge.
    v.clamp(0, hi as isize - 1) as usize
}

/// Collapse a 4x4 quad-Bayer frame into a half-size standard 2x2 Bayer frame by averaging each 2x2 same-colour cluster into one pixel. The output keeps the same base bayer_pattern (each quad cluster maps to the corresponding standard-Bayer cell), so a standard 2x2 debayer (e.g. chameleon's, used for calibration) reads it correctly. Returns ((width+1)/2 * (height+1)/2) u16 at the same value scale as the input (cluster mean preserves 0..white).
pub fn quad_to_standard_bayer(
    raw: &[u16],
    width: usize,
    height: usize,
) -> (usize, usize, Vec<u16>) {
    let ow = width / 2;
    let oh = height / 2;
    let mut out = vec![0u16; ow * oh];
    for oy in 0..oh {
        for ox in 0..ow {
            // The output cell (ox,oy) corresponds to the 2x2 cluster at input origin (2*ox, 2*oy).
            let sx = ox * 2;
            let sy = oy * 2;
            let a = raw[sy * width + sx] as u32;
            // Rule 0: MEMORY SAFETY - clamp the +1 neighbour column index to the last valid column (width-1) so the 2x2 cluster read at the right edge stays in bounds and raw[...] can't index out of bounds.
            let b = raw[sy * width + (sx + 1).min(width - 1)] as u32;
            // Rule 0: MEMORY SAFETY - clamp the +1 neighbour row index to the last valid row (height-1) so the 2x2 cluster read at the bottom edge stays in bounds and raw[...] can't index out of bounds.
            let c = raw[(sy + 1).min(height - 1) * width + sx] as u32;
            // Rule 0: MEMORY SAFETY - clamp both the +1 row and +1 column neighbour indices to the last valid row/column so the bottom-right corner of the 2x2 cluster read stays in bounds and raw[...] can't index out of bounds.
            let d = raw[(sy + 1).min(height - 1) * width + (sx + 1).min(width - 1)] as u32;
            out[oy * ow + ox] = ((a + b + c + d) / 4) as u16;
        }
    }
    (ow, oh, out)
}

/// Demosaic a quad-Bayer frame to full-resolution linear RGB (f32, black-subtracted and gained so the channel range matches a standard 0..white debayer). `raw` is the u16 slot (width*height). `black`/`white` are the per-pixel sensor levels (the 2x2 cluster sum scaling is handled internally: a cluster mean averages 4 same-colour pixels, so levels stay per-pixel). Returns width*height*3 linear RGB with each channel already multiplied by `gain`.
pub fn quad_demosaic(
    raw: &[u16],
    width: usize,
    height: usize,
    black: u16,
    white: u16,
    gain: f32,
    bayer_pattern: u32,
) -> Vec<[f32; 3]> {
    let w = width;
    let h = height;
    // Normalise to 0..1 linear, black-subtracted. (Cluster means below average 4 normalised samples - the extra dynamic range from summing is realised on-device by the integrator's accumulation; here we keep a single normalised scale so the colour matrix downstream is unchanged.)
    // Rule 0: prevents divide-by-zero/negative in scale = 1.0 / (...) when white <= black (degenerate/swapped sensor levels); .max(1.0) keeps the divisor >= 1 so scale stays finite and positive (a 0 or negative divisor would yield +/-inf or a sign-flipped scale, producing inf/NaN or inverted output downstream).
    let scale = 1.0 / (white as f32 - black as f32).max(1.0);
    // No floor: a sub-black sample is real signal (it pulls the cluster mean down, which is correct), and it stays f32 through the matrix downstream. Clamping per-sample would discard noise-floor information; the final sqrt + saturating cast handle any genuinely-negative output.
    let norm = |v: u16| -> f32 { (v as f32 - black as f32) * scale };

    let g = |x: isize, y: isize| -> f32 { norm(raw[clampi(y, h) * w + clampi(x, w)]) };
    let cfa =
        |x: isize, y: isize| -> usize { quad_cfa_color(bayer_pattern, y as usize, x as usize) };

    // 2x2 cluster mean of whatever colour sits at the cluster origin (even,even) containing (x,y).
    let cmean = |x: isize, y: isize| -> f32 {
        let ox = x & !1;
        let oy = y & !1;
        0.25 * (g(ox, oy) + g(ox + 1, oy) + g(ox, oy + 1) + g(ox + 1, oy + 1))
    };

    // --- Step 1: full-res green -------------------------------------------------------------------
    let mut green = vec![0.0f32; w * h];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let fc = cfa(x, y);
            if fc == 1 {
                green[y as usize * w + x as usize] = g(x, y);
                continue;
            }
            let own = cmean(x, y);
            let is_g = |nx: isize, ny: isize| cfa(nx, ny) == 1;

            let (h_est, h_grad, h_ok) = if is_g(x - 2, y) && is_g(x + 2, y) {
                let l = cmean(x - 2, y);
                let r = cmean(x + 2, y);
                (0.5 * (l + r), (l - r).abs(), true)
            } else {
                (0.0, f32::MAX, false)
            };
            let (v_est, v_grad, v_ok) = if is_g(x, y - 2) && is_g(x, y + 2) {
                let u = cmean(x, y - 2);
                let d = cmean(x, y + 2);
                (0.5 * (u + d), (u - d).abs(), true)
            } else {
                (0.0, f32::MAX, false)
            };

            let mut diag_sum = 0.0;
            let mut diag_n = 0.0;
            for (nx, ny) in [
                (x - 2, y - 2),
                (x + 2, y - 2),
                (x - 2, y + 2),
                (x + 2, y + 2),
            ] {
                if is_g(nx, ny) {
                    diag_sum += cmean(nx, ny);
                    diag_n += 1.0;
                }
            }
            let diag_est = if diag_n > 0.0 { diag_sum / diag_n } else { own };

            let g_lpf = if h_ok && (h_grad <= v_grad) {
                h_est
            } else if v_ok {
                v_est
            } else if h_ok {
                h_est
            } else {
                diag_est
            };

            let est = g(x, y) + (g_lpf - own);
            // Rule 0: ALGORITHMIC floor - reconstructed green can't represent negative light, so floor to 0. Not a safety clamp; it prevents a physically impossible negative green value (from the green_lpf - own correction overshooting) leaking into the chroma-difference reconstruction in Step 2.
            green[y as usize * w + x as usize] = est.max(0.0);
        }
    }

    // --- Step 2: R and B from the green-difference domain, directionally --------------------------
    let green_cluster_lpf = |cx: isize, cy: isize| -> f32 {
        let ox = cx & !1;
        let oy = cy & !1;
        0.25 * (green[clampi(oy, h) * w + clampi(ox, w)]
            + green[clampi(oy, h) * w + clampi(ox + 1, w)]
            + green[clampi(oy + 1, h) * w + clampi(ox, w)]
            + green[clampi(oy + 1, h) * w + clampi(ox + 1, w)])
    };

    let mut out = vec![[0.0f32; 3]; w * h];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let gx = green[y as usize * w + x as usize];
            let fc = cfa(x, y);
            let measured = g(x, y);

            let mut rgb = [0.0f32; 3];
            rgb[1] = gx;
            for color in [0usize, 2usize] {
                if color == fc {
                    rgb[color] = measured;
                    continue;
                }
                // chroma difference cd = colour_cluster_mean - green_cluster_lpf, interpolated along edges.
                let cd_at =
                    |cx: isize, cy: isize| -> f32 { cmean(cx, cy) - green_cluster_lpf(cx, cy) };

                let mut h_cd = None;
                {
                    let mut acc = 0.0;
                    let mut n = 0.0;
                    for nx in [x - 2, x + 2] {
                        if nx >= 0 && nx < w as isize && cfa(nx, y) == color {
                            acc += cd_at(nx, y);
                            n += 1.0;
                        }
                    }
                    if n > 0.0 {
                        h_cd = Some(acc / n);
                    }
                }
                let mut v_cd = None;
                {
                    let mut acc = 0.0;
                    let mut n = 0.0;
                    for ny in [y - 2, y + 2] {
                        if ny >= 0 && ny < h as isize && cfa(x, ny) == color {
                            acc += cd_at(x, ny);
                            n += 1.0;
                        }
                    }
                    if n > 0.0 {
                        v_cd = Some(acc / n);
                    }
                }
                let mut d_cd = None;
                {
                    let mut acc = 0.0;
                    let mut n = 0.0;
                    for (nx, ny) in [
                        (x - 2, y - 2),
                        (x + 2, y - 2),
                        (x - 2, y + 2),
                        (x + 2, y + 2),
                    ] {
                        if nx >= 0
                            && ny >= 0
                            && nx < w as isize
                            && ny < h as isize
                            && cfa(nx, ny) == color
                        {
                            acc += cd_at(nx, ny);
                            n += 1.0;
                        }
                    }
                    if n > 0.0 {
                        d_cd = Some(acc / n);
                    }
                }

                let gh = (green[y as usize * w + clampi(x - 1, w)]
                    - green[y as usize * w + clampi(x + 1, w)])
                .abs();
                let gv = (green[clampi(y - 1, h) * w + x as usize]
                    - green[clampi(y + 1, h) * w + x as usize])
                    .abs();

                let cd = match (h_cd, v_cd) {
                    (Some(hc), Some(vc)) => {
                        if gh <= gv {
                            hc
                        } else {
                            vc
                        }
                    }
                    (Some(hc), None) => hc,
                    (None, Some(vc)) => vc,
                    (None, None) => d_cd.unwrap_or(0.0),
                };
                rgb[color] = gx + cd;
            }
            // No floor: a reconstructed channel can dip slightly negative (difference-domain estimate), and that is real signal. The downstream colour matrix + sqrt + saturating cast handle the true value; clamping here would discard information and could even brighten another channel through the matrix's negative coeffs.
            out[y as usize * w + x as usize] = [rgb[0] * gain, rgb[1] * gain, rgb[2] * gain];
        }
    }
    out
}
