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
    let bias_var_raw = load_vsf_field(&args[1], "variance", n);
    let dark_raw = load_vsf_field(&args[2], "mean", n);
    let dark_var = load_vsf_field(&args[2], "variance", n); // frame-to-frame diff, flags unstable pixels
    let light_raw = load_light(&args[3], n);

    // --- DUMP the calibration maps to 16-bit TIFFs (full mosaic, raw stored values) so we can eyeball the
    // actual fixed pattern and make sure we're not chasing ghosts. Each is the raw u16 from the VSF, plus a
    // dark-current map (dark - bias, clamped >=0). Written next to the light. Always on; cheap relative to
    // the rest. ---
    {
        let base = Path::new(&args[3]);
        let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("cal");
        let dir = |suffix: &str| base.with_file_name(format!("{stem}_{suffix}.tiff"));
        write_tiff16(&dir("cal_bias_mean"), w, h, &bias_raw);
        write_tiff16(&dir("cal_bias_var"), w, h, &bias_var_raw);
        write_tiff16(&dir("cal_dark_mean"), w, h, &dark_raw);
        write_tiff16(&dir("cal_dark_var"), w, h, &dark_var);
        // dark current = dark - bias (the signal we'd actually subtract per-second-of-exposure), clamp >=0.
        let darkcur: Vec<u16> = (0..n)
            .map(|i| (dark_raw[i] as i32 - bias_raw[i] as i32).max(0).min(65535) as u16)
            .collect();
        write_tiff16(&dir("cal_dark_current"), w, h, &darkcur);
        println!(
            "dumped cal TIFFs: {}_{{cal_bias_mean,cal_bias_var,cal_dark_mean,cal_dark_var,cal_dark_current}}.tiff",
            stem
        );
    }

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

    // --- THE CORRECTION: per-pixel, on the FULL MOSAIC, BEFORE binning ---
    // The fixed-pattern / popcorn noise is PER-PIXEL. Binning first averages 4 pixels per cluster and smears
    // it away before we can subtract it; that's why the earlier bin-then-subtract did nothing. So we subtract
    // on the raw mosaic, then bin only for display.
    //   bias[i]       = read-noise/offset floor (16s shortest-shutter is ~0s; the bias capture)
    //   darkcurrent[i]= dark[i] - bias[i]  = dark CURRENT alone (per pixel), as captured at the cal exposure
    //   corrected[i]  = light[i] - bias[i] - r * darkcurrent[i]
    // where r scales the dark current from the CAL exposure to the LIGHT's exposure (dark current grows with
    // time). The light's exposure isn't in its metadata (stripped), so assume EXPOSURE_S (default 600s = the
    // ~10 min shot); the dark cal was DARK_CAL_S (default 16s, the forced max shutter). r = EXPOSURE_S/DARK_CAL_S.
    let exposure_s = getenv_f64("EXPOSURE_S", 600.0);
    let dark_cal_s = getenv_f64("DARK_CAL_S", 16.0);
    let r = exposure_s / dark_cal_s;
    println!("exposure assume: light {exposure_s}s / dark-cal {dark_cal_s}s -> dark-current scale r={r:.2}");

    // Reconstruct bad pixels in each map from good same-CFA-phase (stride-4) neighbours so a hot pixel
    // neither subtracts garbage nor poisons its bin.
    let recon = |src: &[f64]| -> Vec<f64> { reconstruct_bad(src, &bad, w, h, 4) };
    let bias_r = recon(&bias_s);
    let dark_r = recon(&dark_s);
    let light_f: Vec<f64> = light_raw.iter().map(|&x| x as f64).collect();
    let light_r = recon(&light_f);

    // corrected mosaic = light - bias - r*(dark - bias)
    let mut corrected = vec![0.0f64; n];
    for i in 0..n {
        let darkcurrent = dark_r[i] - bias_r[i];
        corrected[i] = light_r[i] - bias_r[i] - r * darkcurrent;
    }

    // --- Bin BOTH (light = before, corrected = after) to RGB for viewing ---
    let bayer = getenv_usize("BAYER", 2) as u32; // light CFA GGBB/GGBB/RRGG/RRGG => GBRG = 2
    let black = getenv_f64("BLACK", 4100.0);
    let tw = w / 4;
    let th = h / 4;
    // No-bad mask for binning now (already reconstructed); subtract black so both share the same zero.
    let nobad = vec![false; n];
    let before = bin_quad_rgb_f64(&light_r, &nobad, w, h, bayer, black);
    let after = bin_quad_rgb_f64(&corrected, &nobad, w, h, bayer, black);
    println!("binned to {tw}x{th} RGB ({} px/channel)", tw * th);

    let stem = Path::new(&args[3]).file_stem().and_then(|s| s.to_str()).unwrap_or("light");
    // Shared per-channel min/max over both images.
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for img in [&before, &after] {
        for px in img.iter() {
            for ch in 0..3 {
                lo[ch] = lo[ch].min(px[ch]);
                hi[ch] = hi[ch].max(px[ch]);
            }
        }
    }
    let to16 = |img: &[[f64; 3]]| -> Vec<u16> {
        let mut out = vec![0u16; img.len() * 3];
        for (i, px) in img.iter().enumerate() {
            for ch in 0..3 {
                let span = (hi[ch] - lo[ch]).max(1e-6);
                out[i * 3 + ch] = (((px[ch] - lo[ch]) / span).clamp(0.0, 1.0) * 65535.0).round() as u16;
            }
        }
        out
    };
    let before_path = Path::new(&args[3]).with_file_name(format!("{stem}_binned_before_16.png"));
    let after_path = Path::new(&args[3]).with_file_name(format!("{stem}_binned_after_16.png"));
    write_png16(&before_path, tw, th, &to16(&before));
    write_png16(&after_path, tw, th, &to16(&after));
    println!("\nwrote {} and {} ({}x{}, 16-bit linear, shared scale)", before_path.display(), after_path.display(), tw, th);
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

// Replace each bad (hot/unstable) pixel with the mean of its good same-CFA-phase neighbours at stride
// `period` (N/S/E/W), so a dead pixel never subtracts garbage nor poisons its bin. Good pixels pass through.
fn reconstruct_bad(src: &[f64], bad: &[bool], w: usize, h: usize, period: usize) -> Vec<f64> {
    let mut out = src.to_vec();
    let p = period as i64;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !bad[i] {
                continue;
            }
            let mut s = 0.0;
            let mut c = 0.0;
            let mut acc = |xx: i64, yy: i64| {
                if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                    let j = yy as usize * w + xx as usize;
                    if !bad[j] {
                        s += src[j];
                        c += 1.0;
                    }
                }
            };
            acc(x as i64 - p, y as i64);
            acc(x as i64 + p, y as i64);
            acc(x as i64, y as i64 - p);
            acc(x as i64, y as i64 + p);
            if c > 0.0 {
                out[i] = s / c;
            }
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

#[allow(dead_code)]
fn write_png(path: &Path, w: usize, h: usize, rgb: &[u8]) {
    let f = fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgb).unwrap();
}

// 16-bit RGB PNG. png wants big-endian u16 samples, so serialise each sample as two BE bytes.
fn write_png16(path: &Path, w: usize, h: usize, rgb16: &[u16]) {
    let f = fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Sixteen);
    let mut be = Vec::with_capacity(rgb16.len() * 2);
    for &s in rgb16 {
        be.extend_from_slice(&s.to_be_bytes());
    }
    enc.write_header().unwrap().write_image_data(&be).unwrap();
}

// Minimal uncompressed 16-bit single-channel (grayscale) little-endian TIFF. Hand-rolled: an 8-byte header
// pointing at one IFD, the IFD with the tags a raw-viewer needs, then the pixel data. The cal maps are the
// raw stored u16 values (no scaling) so the TIFF shows EXACTLY what's in the .vsf - the honest fixed-pattern.
fn write_tiff16(path: &Path, w: usize, h: usize, gray16: &[u16]) {
    assert_eq!(gray16.len(), w * h, "tiff size mismatch");
    let entries: u16 = 8;
    let ifd_off: u32 = 8;
    let ifd_bytes = 2 + entries as u32 * 12 + 4;
    let data_off: u32 = ifd_off + ifd_bytes;
    let mut buf: Vec<u8> = Vec::with_capacity(data_off as usize + w * h * 2);
    buf.extend_from_slice(b"II"); // little-endian
    buf.extend_from_slice(&42u16.to_le_bytes()); // magic
    buf.extend_from_slice(&ifd_off.to_le_bytes());
    buf.extend_from_slice(&entries.to_le_bytes());
    // helper to push one 12-byte IFD entry (tag, type, count, value/offset)
    let mut entry = |buf: &mut Vec<u8>, tag: u16, typ: u16, count: u32, val: u32| {
        buf.extend_from_slice(&tag.to_le_bytes());
        buf.extend_from_slice(&typ.to_le_bytes());
        buf.extend_from_slice(&count.to_le_bytes());
        buf.extend_from_slice(&val.to_le_bytes());
    };
    // types: 3=SHORT, 4=LONG. Tags must be ascending.
    entry(&mut buf, 256, 4, 1, w as u32); // ImageWidth
    entry(&mut buf, 257, 4, 1, h as u32); // ImageLength
    entry(&mut buf, 258, 3, 1, 16); // BitsPerSample (SHORT stored in low 2 bytes of the value field)
    entry(&mut buf, 259, 3, 1, 1); // Compression = none
    entry(&mut buf, 262, 3, 1, 1); // PhotometricInterpretation = BlackIsZero
    entry(&mut buf, 273, 4, 1, data_off); // StripOffsets
    entry(&mut buf, 278, 4, 1, h as u32); // RowsPerStrip = whole image
    entry(&mut buf, 279, 4, 1, (w * h * 2) as u32); // StripByteCounts
    buf.extend_from_slice(&0u32.to_le_bytes()); // next IFD = none
    for &v in gray16 {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, &buf).expect("write tiff");
}
