// Host harness to tune the quad-Bayer demosaic against real 16-bit linear Rec.2020 reference images.
//
// Flow: load a reference TIFF (16-bit RGB, linear light) -> forward-model it into a quad-Bayer
// RAW frame (sample each pixel's CFA colour, in a 4x4 Tetracell layout) -> run the REAL
// quad_demosaic -> score reconstruction error -> write an amplified diff PNG showing WHERE it fails.
//
// Error metric is RELATIVE (per-channel |recon - truth| / max(truth, floor)) so a given % error
// counts equally in shadow and highlight - in linear light a flat absolute metric is dominated by
// bright regions and hides shadow demosaic artifacts. We report a relative-error PSNR-like number
// (higher = better) plus mean relative error %.
//
// The real demosaic lives in the shipping source; we include it by path so edits to P/Q are tested
// directly. quad.rs is pure std, so this compiles with no other lumis deps.
#[path = "../../src/debayer/quad.rs"]
mod quad;

use std::path::Path;

// ---- reference TIFF loading (16-bit RGB, linear) ----
fn load_tiff_rgb16(path: &Path) -> (usize, usize, Vec<[u16; 3]>) {
    let file = std::fs::File::open(path).expect("open tiff");
    let mut dec = tiff::decoder::Decoder::new(std::io::BufReader::new(file)).expect("tiff decoder");
    let (w, h) = dec.dimensions().expect("dimensions");
    let img = dec.read_image().expect("read image");
    let w = w as usize;
    let h = h as usize;
    let mut out = vec![[0u16; 3]; w * h];
    match img {
        tiff::decoder::DecodingResult::U16(buf) => {
            // Expect 3 samples/pixel interleaved RGB.
            assert_eq!(buf.len(), w * h * 3, "expected 16-bit RGB, got {} samples", buf.len());
            for i in 0..w * h {
                out[i] = [buf[i * 3], buf[i * 3 + 1], buf[i * 3 + 2]];
            }
        }
        tiff::decoder::DecodingResult::U8(buf) => {
            // Promote 8-bit to 16-bit so the harness still runs (shouldn't happen with these refs).
            assert_eq!(buf.len(), w * h * 3);
            for i in 0..w * h {
                out[i] = [
                    (buf[i * 3] as u16) << 8,
                    (buf[i * 3 + 1] as u16) << 8,
                    (buf[i * 3 + 2] as u16) << 8,
                ];
            }
        }
        other => panic!("unexpected TIFF sample format: {:?}", std::mem::discriminant(&other)),
    }
    (w, h, out)
}

// ---- forward model: reference RGB -> quad-Bayer RAW (u16) ----
// For each pixel, keep only its CFA colour channel (RGGB-base 4x4 Tetracell). black=0, white=65535,
// so the demosaic's normalised 0..1 output maps directly back to truth/65535.
fn mosaic_quad(w: usize, h: usize, rgb: &[[u16; 3]], bayer_pattern: u32) -> Vec<u16> {
    let mut raw = vec![0u16; w * h];
    for y in 0..h {
        for x in 0..w {
            let c = quad::quad_cfa_color(bayer_pattern, y, x);
            raw[y * w + x] = rgb[y * w + x][c];
        }
    }
    raw
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: quad_harness <reference.tif> [more.tif ...]");
        eprintln!("  scores the real quad_demosaic against each reference (16-bit linear Rec.2020 RGB)");
        std::process::exit(2);
    }
    let bayer_pattern = 0u32; // RGGB base

    let mut total_rel = 0.0f64;
    let mut total_psnr = 0.0f64;
    let mut n = 0;

    for path in &args[1..] {
        let p = Path::new(path);
        let (w, h, truth) = load_tiff_rgb16(p);
        let raw = mosaic_quad(w, h, &truth, bayer_pattern);
        // black=0, white=65535, gain=1 -> demosaic returns 0..1 linear matching truth/65535.
        let recon = quad::quad_demosaic(&raw, w, h, 0, 65535, 1.0, bayer_pattern);

        let (rel_pct, psnr, diff) = score_relative(w, h, &truth, &recon);
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
        let diff_path = p.with_file_name(format!("{stem}_diff.png"));
        write_png_rgb8(&diff_path, w, h, &diff);

        println!(
            "{:<28}  {}x{}  rel-err {:>6.3}%   rel-PSNR {:>6.2} dB   -> {}",
            stem,
            w,
            h,
            rel_pct,
            psnr,
            diff_path.file_name().unwrap().to_string_lossy()
        );
        total_rel += rel_pct as f64;
        total_psnr += psnr as f64;
        n += 1;
    }
    if n > 1 {
        println!(
            "{:<28}  mean rel-err {:.3}%   mean rel-PSNR {:.2} dB",
            "ALL", total_rel / n as f64, total_psnr / n as f64
        );
    }
}

// Map a 0..1 linear value to the scoring domain. Two modes (env QH_METRIC):
//   "rel"  (default): identity here; relative error is applied in score_relative (divide by truth).
//   "sqrt": perceptual - sqrt(x + ADDER). The small additive ADDER before the sqrt keeps the
//           slope finite near black (sqrt'(0) is infinite), so deep shadows are not over-amplified
//           - a "slight adder and a sqrt", matching how a display tone curve is usually built.
// The sqrt mode scores ABSOLUTE error in perceptual space (no divide-by-truth), which is a
// fundamentally different, more eye-aligned weighting than relative.
fn perceptual(x: f32) -> f32 {
    const ADDER: f32 = 0.0025; // ~ (5%)^2 ; tweak via QH_ADDER
    let adder = std::env::var("QH_ADDER").ok().and_then(|s| s.parse().ok()).unwrap_or(ADDER);
    (x.max(0.0) + adder).sqrt() - adder.sqrt()
}

fn metric_is_sqrt() -> bool {
    std::env::var("QH_METRIC").map(|m| m == "sqrt").unwrap_or(false)
}

// Edge mask: when QH_EDGES=1, only pixels whose local luma gradient exceeds a threshold are scored.
// P/Q (diagonal green discrimination) only acts on edges, so an edge-restricted score is the
// sensitive way to tune it - flat shadow/sky regions (which dominate the whole-image number but
// P/Q can't change) are excluded.
fn edges_only() -> bool {
    std::env::var("QH_EDGES").map(|e| e == "1").unwrap_or(false)
}

// ---- relative-error scoring ----
// Per channel: e = |recon - truth_norm| / max(truth_norm, FLOOR). FLOOR keeps near-black pixels
// from exploding the metric (a tiny absolute error on a ~0 truth is not a meaningful % error).
// Returns (mean relative error %, relative PSNR dB, amplified diff RGB8).
fn score_relative(w: usize, h: usize, truth: &[[u16; 3]], recon: &[[f32; 3]]) -> (f32, f32, Vec<u8>) {
    // Floor at ~2% of full scale (perceptual noise level for a 16-bit linear sensor). Below this,
    // deep-shadow error is below visible noise and dividing by near-zero truth would let invisible
    // shadow noise dominate the score. Flooring here keeps the relative metric meaningful where it
    // matters - midtones and highlights, where demosaic edge artifacts (and P/Q) actually live.
    const FLOOR: f32 = 0.02;
    const DIFF_GAIN: f32 = 20.0; // amplify the diff image so artifacts are visible
    let use_sqrt = metric_is_sqrt();
    let use_edges = edges_only();
    let luma = |p: [u16; 3]| -> f32 {
        (p[0] as f32 * 0.2627 + p[1] as f32 * 0.6780 + p[2] as f32 * 0.0593) / 65535.0
    };
    // Edge threshold on |gradient| of truth luma (per full scale). ~3% picks out real edges.
    let edge_thresh = std::env::var("QH_EDGE_THRESH").ok().and_then(|s| s.parse().ok()).unwrap_or(0.03f32);

    let mut sum_sq = 0.0f64;
    let mut sum_abs = 0.0f64;
    let mut count = 0.0f64;
    let mut diff = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            // Skip non-edge pixels when edge-masking. Central-difference luma gradient magnitude.
            if use_edges {
                let xm = x.saturating_sub(1);
                let xp = (x + 1).min(w - 1);
                let ym = y.saturating_sub(1);
                let yp = (y + 1).min(h - 1);
                let gx = (luma(truth[y * w + xp]) - luma(truth[y * w + xm])).abs();
                let gy = (luma(truth[yp * w + x]) - luma(truth[ym * w + x])).abs();
                if gx + gy < edge_thresh {
                    continue;
                }
            }
            for c in 0..3 {
                let t = truth[i][c] as f32 / 65535.0;
                let r = recon[i][c].max(0.0);
                let e = if use_sqrt {
                    // Absolute error in perceptual (sqrt+adder) space.
                    (perceptual(r) - perceptual(t)).abs()
                } else {
                    // Relative error, floored.
                    (r - t).abs() / t.max(FLOOR)
                };
                sum_sq += (e as f64) * (e as f64);
                sum_abs += e as f64;
                count += 1.0;
                // amplified absolute diff for the visual (so the eye sees structure, not relative blowups)
                let d = ((r - t).abs() * DIFF_GAIN * 255.0).min(255.0);
                diff[i * 3 + c] = d as u8;
            }
        }
    }
    if count == 0.0 {
        return (0.0, 99.0, diff);
    }
    let mean_rel = (sum_abs / count) as f32;
    let rms_rel = (sum_sq / count).sqrt() as f32;
    // PSNR-like: 20*log10(1 / rms_relative_error). Reference signal level is 1 (100%).
    let psnr = if rms_rel > 0.0 { 20.0 * (1.0 / rms_rel).log10() } else { 99.0 };

    // Diagnostic: split mean relative error by truth luma band so we see WHERE error lives.
    // If it's all in the shadow band, the metric is floor-dominated, not a real demosaic problem.
    let mut band_abs = [0.0f64; 3]; // shadow / mid / highlight
    let mut band_n = [0.0f64; 3];
    for i in 0..w * h {
        let luma = (truth[i][0] as f32 * 0.2627 + truth[i][1] as f32 * 0.6780 + truth[i][2] as f32 * 0.0593) / 65535.0;
        let band = if luma < 0.05 { 0 } else if luma < 0.5 { 1 } else { 2 };
        for c in 0..3 {
            let t = truth[i][c] as f32 / 65535.0;
            let r = recon[i][c].max(0.0);
            let e = (r - t).abs() / t.max(FLOOR);
            band_abs[band] += e as f64;
            band_n[band] += 1.0;
        }
    }
    let pc = |k: usize| if band_n[k] > 0.0 { 100.0 * band_abs[k] / band_n[k] } else { 0.0 };
    eprintln!(
        "    bands rel-err: shadow {:.2}% ({:.0}%px)  mid {:.2}%  highlight {:.2}%",
        pc(0), 100.0 * band_n[0] / (count), pc(1), pc(2)
    );
    (mean_rel * 100.0, psnr, diff)
}

fn write_png_rgb8(path: &Path, w: usize, h: usize, rgb: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("png header");
    writer.write_image_data(rgb).expect("png data");
}
