//! Dark-frame calibration alpha/beta solver harness (host-side), BINNED-RGB edition.
//!
//! The sensor is a 4x4 quad-Bayer (Tetracell): a plain neighbourhood metric on the raw mosaic mixes
//! R/G/B and CFA-phase, drowning the per-pixel fixed pattern in colour structure. Instead we do what the
//! device already does for previews: STRICTLY BIN each 4x4 tile to one linear-RGB pixel (no interpolation,
//! just same-colour cluster means), masking dead pixels out of their cluster first. After binning there's
//! no CFA phase left - each output pixel is a clean per-channel value and the sensor's fixed-pattern noise
//! survives as a per-output-pixel residual. We then solve, per channel:
//!     corrected = light_rgb - (alpha*bias_fp + beta*dark_fp)
//! where *_fp = (binned cal) minus its low-freq (a 3x3 box on the BINNED image, now colour-safe). alpha and
//! beta come from a least-squares regression of the light's own high-freq onto the templates over dark,
//! good pixels (random noise averages out of the dot products; only correlated FPN survives). Bonus: the
//! fit runs on n/16 pixels, so it's ~16x faster than the per-pixel mosaic version.
//!
//! Usage:  cal_harness <bias.vsf> <dark.vsf> <light.dng|.bin> <W> <H>
//! Env:    CAL_DNG_OFFSET (raw strip byte offset), CAL_SCALE (cal->light bit scale, default 64=<<6),
//!         HOT_LEVEL / HOT_DIFF (dead-pixel thresholds, 16-bit), BAYER (base pattern 0..3, default 2=GBRG),
//!         BLACK (light black level, default 4100), CAL_FLOOR (dark-region mask on the binned light).
//! Writes <light>_before.png / <light>_after.png (the binned RGB, before vs after correction).

use std::env;
use std::fs;
use std::path::Path;

fn getenv_f64(k: &str, d: f64) -> f64 {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn getenv_usize(k: &str, d: usize) -> usize {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 6 {
        eprintln!("usage: cal_harness <bias.vsf> <dark.vsf> <light.dng|.bin> <W> <H>");
        std::process::exit(2);
    }
    let w: usize = args[4].parse().unwrap();
    let h: usize = args[5].parse().unwrap();
    let n = w * h;

    // --- Load raw maps (full-res mosaic, u16) ---
    let bias_raw = load_vsf_field(&args[1], "mean", n);
    let dark_raw = load_vsf_field(&args[2], "mean", n);
    let dark_var = load_vsf_field(&args[2], "variance", n); // frame-to-frame diff, flags unstable pixels
    let light_raw = load_light(&args[3], n);

    // Cal maps are raw 10-bit DN; the light DNG is the same data shifted to 16-bit (<<6 = x64). Bring the
    // cal into the light's scale so a binned cal pixel matches a binned light pixel.
    let cal_scale = getenv_f64("CAL_SCALE", 64.0);
    let bias_s: Vec<f64> = bias_raw.iter().map(|&x| x as f64 * cal_scale).collect();
    let dark_s: Vec<f64> = dark_raw.iter().map(|&x| x as f64 * cal_scale).collect();
    let dvar_s: Vec<f64> = dark_var.iter().map(|&x| x as f64 * cal_scale).collect();

    // --- Dead/hot-pixel mask (mask BEFORE binning so a hot pixel never poisons its tile) ---
    // Bad if the dark adder is too hot (level) OR its frame-to-frame diff is too unstable. Defaults sit well
    // above each map's bulk; tune via env.
    let hot_level = getenv_f64("HOT_LEVEL", 9000.0);
    let hot_diff = getenv_f64("HOT_DIFF", 8000.0);
    let bad: Vec<bool> = (0..n).map(|i| dark_s[i] > hot_level || dvar_s[i] > hot_diff).collect();
    let n_bad = bad.iter().filter(|&&b| b).count();
    println!(
        "loaded {w}x{h} = {n} px; cal scaled x{:.0}; dead-pixel mask: level>{:.0} OR diff>{:.0} -> {} bad ({:.4}%)",
        cal_scale, hot_level, hot_diff, n_bad, 100.0 * n_bad as f64 / n as f64
    );

    // --- Strict 4x4 quad-Bayer bin -> linear RGB (dead-pixel-aware) ---
    let bayer = getenv_usize("BAYER", 2) as u32; // light CFA GGBB/GGBB/RRGG/RRGG => GBRG = 2
    let black = getenv_f64("BLACK", 4100.0);
    let tw = w / 4;
    let th = h / 4;
    let light_rgb = bin_quad_rgb_f64(&light_raw.iter().map(|&x| x as f64).collect::<Vec<_>>(), &bad, w, h, bayer, black);
    // The cal maps already have their own offset baked in; bin them with black=0 (we high-pass them next,
    // which removes any DC anyway). Same dead-pixel mask.
    let bias_rgb = bin_quad_rgb_f64(&bias_s, &bad, w, h, bayer, 0.0);
    let dark_rgb = bin_quad_rgb_f64(&dark_s, &bad, w, h, bayer, 0.0);
    println!("binned to {tw}x{th} RGB ({} px/channel)", tw * th);

    // --- High-pass each binned channel (3x3 box on the BINNED image; colour-safe now) ---
    // *_fp = binned - box3x3(binned). Removes lens-shading / smooth gradients, leaving per-pixel structure.
    let bias_fp = highpass_rgb(&bias_rgb, tw, th);
    let dark_fp = highpass_rgb(&dark_rgb, tw, th);
    let light_fp = highpass_rgb(&light_rgb, tw, th);

    // Dark-region mask on the binned light: only fit where the scene is near black (FPN dominates there;
    // bright scene edges would swamp it). floor is in the binned cluster-sum scale (4 px summed per cluster).
    let floor = getenv_f64("CAL_FLOOR", 2000.0);

    // --- Per-channel least-squares regression: light_fp ~ alpha*bias_fp + beta*dark_fp ---
    let chan_name = ["R", "G", "B"];
    let mut alpha = [0.0f64; 3];
    let mut beta = [0.0f64; 3];
    for ch in 0..3 {
        let (mut sbb, mut sbd, mut sdd, mut sbl, mut sdl, mut sll) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        let mut nfit = 0u64;
        for i in 0..tw * th {
            // skip bright scene (use the binned light's channel level vs floor) and tiles that came out as
            // exact zero (all-dead cluster -> no data).
            if light_rgb[i][ch] > floor {
                continue;
            }
            let b = bias_fp[i][ch];
            let d = dark_fp[i][ch];
            let l = light_fp[i][ch];
            sbb += b * b;
            sbd += b * d;
            sdd += d * d;
            sbl += b * l;
            sdl += d * l;
            sll += l * l;
            nfit += 1;
        }
        let det = sbb * sdd - sbd * sbd;
        let (a, bt) = if det.abs() > 1e-9 {
            ((sbl * sdd - sdl * sbd) / det, (sbb * sdl - sbd * sbl) / det)
        } else {
            (0.0, 0.0)
        };
        alpha[ch] = a;
        beta[ch] = bt;
        let corr_bias = sbl / (sbb.sqrt() * sll.sqrt()).max(1e-9);
        let corr_dark = sdl / (sdd.sqrt() * sll.sqrt()).max(1e-9);
        let explained = (a * sbl + bt * sdl) / sll.max(1e-9);
        println!(
            "  [{}] over {:>8} dark px:  corr bias={:+.4} dark={:+.4}  ->  alpha={:+.4} beta={:+.4}  (R^2={:.4}, {:.1}%)",
            chan_name[ch], nfit, corr_bias, corr_dark, a, bt, explained, 100.0 * explained
        );
    }

    // --- Apply + write before/after PNGs (8-bit, auto-scaled per image for eyeballing) ---
    let stem = Path::new(&args[3]).file_stem().and_then(|s| s.to_str()).unwrap_or("light");
    let before = render_rgb(&light_rgb, &bias_fp, &dark_fp, tw, th, &[0.0; 3], &[0.0; 3]);
    let after = render_rgb(&light_rgb, &bias_fp, &dark_fp, tw, th, &alpha, &beta);
    write_png(&Path::new(&args[3]).with_file_name(format!("{stem}_binned_before.png")), tw, th, &before);
    write_png(&Path::new(&args[3]).with_file_name(format!("{stem}_binned_after.png")), tw, th, &after);
    println!("\nwrote {stem}_binned_before.png / {stem}_binned_after.png ({tw}x{th})");
}

// Base 2x2 cluster colours (0=R,1=G,2=B) for the Android base bayer pattern, same mapping the device uses.
fn base_2x2(bayer_pattern: u32) -> [usize; 4] {
    match bayer_pattern {
        0 => [0, 1, 1, 2], // RGGB
        1 => [1, 0, 2, 1], // GRBG
        2 => [1, 2, 0, 1], // GBRG
        3 => [2, 1, 1, 0], // BGGR
        _ => [0, 1, 1, 2],
    }
}

// Strict quad-Bayer bin to linear RGB, dead-pixel-aware. One RGB pixel per 4x4 tile: each 2x2 cluster is
// the MEAN of its live (non-dead) same-colour samples (dead pixels excluded, not summed as black); the two
// green clusters are averaged. Mirrors the device's bin_quad_rgb but operates on f64 and skips dead pixels.
fn bin_quad_rgb_f64(raw: &[f64], bad: &[bool], width: usize, height: usize, bayer: u32, black: f64) -> Vec<[f64; 3]> {
    let tw = width / 4;
    let th = height / 4;
    let mut out = vec![[0.0f64; 3]; tw * th];
    let base = base_2x2(bayer);
    // Mean of the live samples in the 2x2 cluster at tile origin (ox,oy) + cluster offset (cx,cy).
    let cluster = |ox: usize, oy: usize, cx: usize, cy: usize| -> f64 {
        let mut s = 0.0;
        let mut c = 0.0;
        for dy in 0..2 {
            for dx in 0..2 {
                let idx = (oy + cy + dy) * width + (ox + cx + dx);
                if !bad[idx] {
                    s += raw[idx] - black;
                    c += 1.0;
                }
            }
        }
        if c > 0.0 { s / c } else { 0.0 } // all-dead cluster -> 0 (rare; masked by the dark-region/skip logic)
    };
    for ty in 0..th {
        for tx in 0..tw {
            let ox = tx * 4;
            let oy = ty * 4;
            let mut rgb = [0.0f64; 3];
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
    out
}

// High-pass each channel of a binned RGB image: value minus its 3x3 box mean. Colour-safe because the
// image is already de-mosaiced (one RGB per pixel). Leaves per-pixel structure (FPN + random noise).
fn highpass_rgb(img: &[[f64; 3]], w: usize, h: usize) -> Vec<[f64; 3]> {
    let mut out = vec![[0.0f64; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            for ch in 0..3 {
                let mut s = 0.0f64;
                let mut c = 0.0f64;
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let xx = x as i64 + dx;
                        let yy = y as i64 + dy;
                        if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                            s += img[yy as usize * w + xx as usize][ch];
                            c += 1.0;
                        }
                    }
                }
                out[i][ch] = img[i][ch] - s / c.max(1.0);
            }
        }
    }
    out
}

// Render corrected = light_rgb - (alpha*bias_fp + beta*dark_fp) to an 8-bit RGB PNG, auto-scaled per image
// (min..max -> 0..255 with a single sqrt gamma) so the structure is visible for eyeballing.
fn render_rgb(light: &[[f64; 3]], bias_fp: &[[f64; 3]], dark_fp: &[[f64; 3]], w: usize, h: usize, alpha: &[f64; 3], beta: &[f64; 3]) -> Vec<u8> {
    let n = w * h;
    let mut vals = vec![[0.0f64; 3]; n];
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for i in 0..n {
        for ch in 0..3 {
            let c = light[i][ch] - (alpha[ch] * bias_fp[i][ch] + beta[ch] * dark_fp[i][ch]);
            vals[i][ch] = c;
            if c < lo[ch] {
                lo[ch] = c;
            }
            if c > hi[ch] {
                hi[ch] = c;
            }
        }
    }
    let mut out = vec![0u8; n * 3];
    for i in 0..n {
        for ch in 0..3 {
            let span = (hi[ch] - lo[ch]).max(1e-6);
            let v = ((vals[i][ch] - lo[ch]) / span).clamp(0.0, 1.0);
            out[i * 3 + ch] = (v.sqrt() * 255.0) as u8;
        }
    }
    out
}

// ---- loaders / io (unchanged) ----

fn load_vsf_field(path: &str, field: &str, n: usize) -> Vec<u16> {
    use vsf::file_format::{VsfHeader, VsfSection};
    use vsf::types::VsfType;
    let data = fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let (header, _) = VsfHeader::decode(&data).unwrap_or_else(|e| panic!("{path} VSF decode/verify: {e}"));
    let f = header
        .fields
        .iter()
        .find(|f| f.name == "calibration")
        .unwrap_or_else(|| panic!("{path}: no 'calibration' section"));
    let mut ptr = f.offset_bytes;
    let section = VsfSection::parse(&data, &mut ptr).unwrap_or_else(|e| panic!("{path} section parse: {e}"));
    let v = section
        .get_field(field)
        .and_then(|fld| fld.values.first())
        .unwrap_or_else(|| panic!("{path}: no '{field}' field"));
    match v {
        VsfType::v(b'z', compressed) => {
            use flate2::read::ZlibDecoder;
            use std::io::Read;
            let mut dec = ZlibDecoder::new(&compressed[..]);
            let mut raw = Vec::new();
            dec.read_to_end(&mut raw).expect("inflate cal map");
            assert_eq!(raw.len(), n * 2, "{path} {field}: {} bytes, expected {}", raw.len(), n * 2);
            (0..n).map(|i| u16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]])).collect()
        }
        VsfType::p(tensor) => {
            let u = tensor.unpack_u16();
            assert_eq!(u.len(), n, "{path} {field}: {} px, expected {}", u.len(), n);
            u
        }
        _ => panic!("{path}: '{field}' is not a compressed map or tensor"),
    }
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

// Load the light frame: raw .bin (u16 LE) or a DNG strip (offset via CAL_DNG_OFFSET, else final n*2 bytes).
fn load_light(path: &str, n: usize) -> Vec<u16> {
    if path.ends_with(".bin") {
        return load_u16(path, n);
    }
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

fn write_png(path: &Path, w: usize, h: usize, rgb: &[u8]) {
    let f = fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgb).unwrap();
}
