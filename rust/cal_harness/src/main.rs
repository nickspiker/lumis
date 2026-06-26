//! Dark-frame calibration alpha/beta solver harness (host-side).
//!
//! Model: corrected = light - (alpha*bias + beta*dark), per pixel.
//! - bias  = read-noise/offset floor (max ISO, shortest shutter), as a per-pixel mean map.
//! - dark  = dark-current + bias (max ISO, longest shutter), as a per-pixel mean map.
//! - light = an actual raw frame to correct (u16, the DNG's raw strip).
//!
//! Objective: drive the FIXED-PATTERN noise to zero. Fixed-pattern noise is the per-pixel structure that
//! does NOT vary smoothly across the image, so it survives a spatial smoothing as a residual. We minimise
//! the energy of (corrected - smoothed(corrected)) over alpha/beta. The right alpha/beta cancel the
//! sensor's pattern, leaving only the smoothly-varying real scene (and irreducible random noise).
//!
//! Usage:
//!   cal_harness <bias.bin> <dark.bin> <light.dng> <W> <H>
//! Writes before/after PNGs next to the light file and prints the alpha/beta grid + best fit.

use std::env;
use std::fs;
use std::path::Path;

const STRIDE: usize = 9; // sample stride for the fast residual metric (every 9th pixel is plenty at 50MP)

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 6 {
        eprintln!("usage: cal_harness <bias.bin> <dark.bin> <light.dng|.bin> <W> <H>");
        std::process::exit(2);
    }
    let w: usize = args[4].parse().unwrap();
    let h: usize = args[5].parse().unwrap();
    let n = w * h;

    let bias = load_u16(&args[1], n);
    let dark = load_u16(&args[2], n);
    let light = load_light(&args[3], n);
    println!("loaded {}x{} = {} px", w, h, n);
    stats("bias", &bias);
    stats("dark", &dark);
    stats("light", &light);

    // --- Solve alpha/beta by minimising the fixed-pattern residual ---
    // Coarse grid, then a local refine. Residual = sum( (corrected - smooth(corrected))^2 ) over a sample.
    let mut best = (1.0f64, 1.0f64, f64::MAX);
    println!("\ncoarse grid (alpha, beta -> residual):");
    let grid: Vec<f64> = (0..=20).map(|i| i as f64 * 0.1).collect(); // 0.0 .. 2.0 step 0.1
    for &a in &grid {
        for &b in &grid {
            let r = residual(&light, &bias, &dark, w, h, a, b);
            if r < best.2 {
                best = (a, b, r);
            }
        }
    }
    println!("  coarse best: alpha={:.2} beta={:.2} residual={:.1}", best.0, best.1, best.2);

    // Local refine around the coarse best (step 0.02, +/-0.1).
    let (ca, cb, _) = best;
    let mut refined = best;
    let mut a = ca - 0.1;
    while a <= ca + 0.1 + 1e-9 {
        let mut b = cb - 0.1;
        while b <= cb + 0.1 + 1e-9 {
            if a >= 0.0 && b >= 0.0 {
                let r = residual(&light, &bias, &dark, w, h, a, b);
                if r < refined.2 {
                    refined = (a, b, r);
                }
            }
            b += 0.02;
        }
        a += 0.02;
    }
    let (alpha, beta, res) = refined;
    println!("  refined best: alpha={:.3} beta={:.3} residual={:.1}", alpha, beta, res);

    // Baseline residuals for comparison.
    let r_none = residual(&light, &bias, &dark, w, h, 0.0, 0.0);
    let r_naive = residual(&light, &bias, &dark, w, h, 1.0, 1.0); // physical estimate (alpha=beta=1)
    println!("\nresidual comparison (lower = flatter / less fixed-pattern):");
    println!("  no correction      (a=0,   b=0  ): {:.1}", r_none);
    println!("  naive physical     (a=1,   b=1  ): {:.1}", r_naive);
    println!("  solved             (a={:.2}, b={:.2}): {:.1}", alpha, beta, res);
    println!("  improvement vs none:  {:.1}%", 100.0 * (r_none - res) / r_none);
    println!("  improvement vs naive: {:.1}%", 100.0 * (r_naive - res) / r_naive.max(1.0));

    // --- Write before/after preview PNGs (mean-subtracted, gamma-2, so the pattern is visible) ---
    let stem = Path::new(&args[3]).file_stem().and_then(|s| s.to_str()).unwrap_or("light");
    let before = render_preview(&light, &bias, &dark, w, h, 0.0, 0.0);
    let after = render_preview(&light, &bias, &dark, w, h, alpha, beta);
    write_png(&Path::new(&args[3]).with_file_name(format!("{stem}_before.png")), w, h, &before);
    write_png(&Path::new(&args[3]).with_file_name(format!("{stem}_after.png")), w, h, &after);
    println!("\nwrote {stem}_before.png / {stem}_after.png (mean-subtracted, amplified - inspect the noise)");
}

// corrected = light - (alpha*bias + beta*dark), as f64 (can go negative; that's real signal).
#[inline]
fn corrected(light: u16, bias: u16, dark: u16, a: f64, b: f64) -> f64 {
    light as f64 - (a * bias as f64 + b * dark as f64)
}

// Fixed-pattern residual: sum of (corrected - local_mean(corrected))^2 over a sampled set of pixels.
// local_mean is a cheap 3x3 box (the smoothing); the high-freq fixed pattern is what's left after it.
fn residual(light: &[u16], bias: &[u16], dark: &[u16], w: usize, h: usize, a: f64, b: f64) -> f64 {
    let at = |x: usize, y: usize| corrected(light[y * w + x], bias[y * w + x], dark[y * w + x], a, b);
    let mut sum = 0.0f64;
    let mut count = 0u64;
    let mut idx = 0usize;
    // Walk interior pixels (need the 3x3 neighbourhood), sampled by STRIDE.
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            idx += 1;
            if idx % STRIDE != 0 {
                continue;
            }
            let c = at(x, y);
            // 3x3 mean.
            let mut m = 0.0;
            for dy in 0..3 {
                for dx in 0..3 {
                    m += at(x + dx - 1, y + dy - 1);
                }
            }
            m /= 9.0;
            let d = c - m;
            sum += d * d;
            count += 1;
        }
    }
    sum / count.max(1) as f64
}

// Mean-subtracted, gamma-2, fixed-range preview so the fixed-pattern noise is visible (like the on-device
// dark preview). Amplified so small residual structure shows.
fn render_preview(light: &[u16], bias: &[u16], dark: &[u16], w: usize, h: usize, a: f64, b: f64) -> Vec<u8> {
    let n = w * h;
    let mut vals = vec![0.0f64; n];
    let mut sum = 0.0;
    for i in 0..n {
        let c = corrected(light[i], bias[i], dark[i], a, b);
        vals[i] = c;
        sum += c;
    }
    let mean = sum / n as f64;
    // Amplify the residual: scale so +/- ~4*noise fills the range. Use a fixed gain that makes structure pop.
    const GAIN: f64 = 6.0;
    let span = 1024.0; // fixed display span in raw counts; tuned to make pattern visible
    let mut out = vec![0u8; n * 3];
    for i in 0..n {
        let v = ((vals[i] - mean) * GAIN / span + 0.5).clamp(0.0, 1.0); // centre at mid-grey
        let g = (v.sqrt() * 255.0) as u8;
        out[i * 3] = g;
        out[i * 3 + 1] = g;
        out[i * 3 + 2] = g;
    }
    out
}

fn load_u16(path: &str, n: usize) -> Vec<u16> {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    assert!(bytes.len() >= n * 2, "{path}: {} bytes, need {}", bytes.len(), n * 2);
    let mut v = vec![0u16; n];
    for i in 0..n {
        v[i] = u16::from_le_bytes([bytes[i * 2], bytes[i * 2 + 1]]);
    }
    v
}

// Load the light frame: either a raw .bin (u16 LE) or a DNG (find the uncompressed strip). For a DNG we
// locate the largest IFD strip of size n*2 bytes - good enough for our own single-strip DNGs.
fn load_light(path: &str, n: usize) -> Vec<u16> {
    if path.ends_with(".bin") {
        return load_u16(path, n);
    }
    // DNG: the raw strip is n*2 contiguous bytes. We don't parse the IFD here; instead scan for the strip
    // by trying the known StripOffsets convention won't generalise, so require the caller to pass the
    // offset via env CAL_DNG_OFFSET, else assume the strip is the final n*2 bytes (our DNGs append raw last
    // before any trailing IFD - if that fails, pass the offset explicitly).
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let off: usize = std::env::var("CAL_DNG_OFFSET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| bytes.len().saturating_sub(n * 2));
    assert!(off + n * 2 <= bytes.len(), "DNG offset {off} + {} > {} - pass CAL_DNG_OFFSET", n * 2, bytes.len());
    let mut v = vec![0u16; n];
    for i in 0..n {
        v[i] = u16::from_le_bytes([bytes[off + i * 2], bytes[off + i * 2 + 1]]);
    }
    v
}

fn stats(name: &str, v: &[u16]) {
    let mut mn = u16::MAX;
    let mut mx = 0u16;
    let mut s = 0.0f64;
    for &x in v.iter().step_by(7) {
        mn = mn.min(x);
        mx = mx.max(x);
        s += x as f64;
    }
    let cnt = (v.len() + 6) / 7;
    println!("  {name:6} min={mn} max={mx} mean={:.1}", s / cnt as f64);
}

fn write_png(path: &Path, w: usize, h: usize, rgb: &[u8]) {
    let f = fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgb).unwrap();
}
