//! Dark-frame calibration: load a cal .vsf (bias + dark maps + metadata) and apply it to a light frame.
//!
//! Model (agreed with the math worked through by hand), per pixel, in the SAME 16-bit-scaled domain the
//! final DNG uses (cal maps are raw 10-bit DN; we scale them by `cal_scale` = white_light/white_cal to match
//! the light):
//!
//!   g   = iso_light / iso_cal              gain ratio. The per-pixel offset (FPN) is ANALOG, pre-A/D, so it
//!                                          is multiplied by the sensor gain - both terms scale with g.
//!   te  = t_light  / t_dark_cal            exposure ratio. Dark current accumulates with time.
//!
//!   offset_term      = (bias[i] - black) * g           per-pixel offset PATTERN (black pedestal kept)
//!   darkcurrent_term = (dark[i] - bias[i]) * g * te     dark current alone (black cancels in dark-bias)
//!   corrected[i]     = light[i] - offset_term - darkcurrent_term
//!
//! The flat black pedestal is deliberately LEFT in (only the per-pixel pattern is removed) so the DNG keeps its normal BlackLevel tag and downstream demosaicers behave.
//!
//! Hot/unstable pixels (flagged by the dark level OR its frame-to-frame variance) are reconstructed from good same-CFA-phase neighbours after the subtraction, since subtracting a blown/noisy cal value can't recover a bad pixel.
//!
//! iso_cal / exposure_ns / black_level come from the cal VSF itself (stored at finalize), never re-queried
//! from the sensor - a firmware/HAL update can move the ISO range, which would corrupt g if we trusted the
//! current max instead of the concrete value the cal was captured at.

use vsf::file_format::{VsfHeader, VsfSection};
use vsf::types::VsfType;

/// One decoded cal .vsf file: its mean + variance maps and metadata. A `bias` file and a `dark` file are
/// each decoded into one of these, then combined via `LoadedCalibration::from_pair`.
pub struct CalFile {
    pub width: usize,
    pub height: usize,
    pub iso_cal: f64,
    pub exposure_ns: f64,
    pub black_level: f64,
    pub mean: Vec<u16>,
    pub variance: Option<Vec<u16>>, // only inflated when needed (the dark file, for the bad-pixel mask)
}

impl CalFile {
    /// Decode + checksum-verify a single cal VSF into its mean (+ optionally variance) map + metadata. None
    /// on any structural/decode/size mismatch. `want_variance` skips inflating the ~100MB variance map when
    /// the caller won't use it (the bias file) - the camera process can't afford the extra copy.
    pub fn decode(bytes: &[u8], want_variance: bool) -> Option<CalFile> {
        let header = VsfHeader::decode(bytes).ok()?.0;
        let field = header.fields.iter().find(|f| f.name == "calibration")?;
        let mut ptr = field.offset_bytes;
        let section = VsfSection::parse(bytes, &mut ptr).ok()?;

        let get_u64 = |name: &str| -> Option<u64> {
            match section.get_field(name).and_then(|f| f.values.first()) {
                Some(VsfType::u6(v)) => Some(*v),
                _ => None,
            }
        };
        let width = get_u64("width")? as usize;
        let height = get_u64("height")? as usize;
        let n = width * height;
        // Metadata with sane fallbacks for older cals that predate these fields.
        let iso_cal = get_u64("iso_cal").unwrap_or(0) as f64;
        let exposure_ns = get_u64("exposure_ns").unwrap_or(0) as f64;
        let black_level = get_u64("black_level").unwrap_or(0) as f64;

        let inflate = |name: &str| -> Option<Vec<u16>> {
            match section.get_field(name).and_then(|f| f.values.first()) {
                Some(VsfType::v(b'z', compressed)) => {
                    use flate2::read::ZlibDecoder;
                    use std::io::Read;
                    let mut dec = ZlibDecoder::new(&compressed[..]);
                    let mut raw = Vec::new();
                    dec.read_to_end(&mut raw).ok()?;
                    // Validating UNTRUSTED external data: this is a deflate blob from a .vsf file on disk that
                    // could be corrupt or truncated. A short/long inflate would make the index below read OOB,
                    // so reject it as a decode failure (None -> caller falls back to no correction). This
                    // guards real untrusted input, not a guaranteed-correct internal buffer.
                    if raw.len() != n * 2 {
                        return None;
                    }
                    Some((0..n).map(|i| u16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]])).collect())
                }
                _ => None,
            }
        };
        let mean = inflate("mean")?;
        let variance = if want_variance { Some(inflate("variance")?) } else { None };
        Some(CalFile { width, height, iso_cal, exposure_ns, black_level, mean, variance })
    }
}

/// A decoded, ready-to-apply calibration (bias map + dark map + metadata). Maps are raw 10-bit DN (as
/// stored); scaling to the light's domain happens in `apply`.
pub struct LoadedCalibration {
    pub width: usize,
    pub height: usize,
    pub iso_cal: f64,
    pub exposure_ns: f64,
    pub black_level: f64,
    pub bias: Vec<u16>, // per-pixel mean, max-ISO shortest-shutter
    // per-pixel dark mean (max-ISO longest-shutter = bias + dark current). Hot/unstable pixels are marked IN PLACE with the sentinel u16::MAX (cal maps are raw 10-bit, max ~1023, so MAX is unambiguous) - no separate mask Vec. apply() reconstructs any pixel whose dark == MAX.
    pub dark: Vec<u16>,
}

/// Sentinel marking a hot/unstable pixel in the dark map. Cal data is raw 10-bit (0..~1023), so u16::MAX never collides with a real value.
pub const BAD: u16 = u16::MAX;

impl LoadedCalibration {
    /// Combine a decoded BIAS file and a decoded DARK file into a ready calibration. Dimensions must match.
    /// Metadata (iso_cal/exposure_ns/black_level) comes from the DARK file (its exposure_ns = the 16s the
    /// dark current was integrated over). The DARK file must carry its variance map (decode with want_variance=true) so we can flag hot/unstable pixels.
    ///
    /// CONSUMES both CalFiles and MOVES their mean Vecs (no clone - cloning two 50MP maps on the camera process blew the memory budget and got it OS-killed). Outlier (hot/unstable) pixels are marked IN the dark map as BAD (u16::MAX); the dark variance is consumed to find them and then dropped, so only bias + dark stay resident - no separate mask allocation.
    pub fn from_pair(bias_file: CalFile, mut dark_file: CalFile) -> Option<LoadedCalibration> {
        if bias_file.width != dark_file.width || bias_file.height != dark_file.height {
            return None;
        }
        Self::mark_bad_pixels(&mut dark_file.mean, dark_file.variance.as_deref());
        Some(LoadedCalibration {
            width: dark_file.width,
            height: dark_file.height,
            iso_cal: dark_file.iso_cal,
            exposure_ns: dark_file.exposure_ns,
            black_level: dark_file.black_level,
            bias: bias_file.mean, // moved, not cloned
            dark: dark_file.mean, // moved, not cloned (outliers now == BAD)
        })
    }

    /// Mark hot/unstable pixels in the dark map IN PLACE as BAD (u16::MAX), using a robust, self-adapting outlier test: median + N*1.4826*MAD on the dark LEVEL, and (if present) the same on the dark VARIANCE. MAD (median absolute deviation) is outlier-resistant - the hot pixels don't inflate it the way std would - and median+MAD adapts to whatever sensor/ISO/exposure the cal was shot at, so there are no magic DN constants. A pixel is bad if it's a level outlier OR a variance outlier. Stats are taken over a strided sample (a few hundred k pixels is plenty for a stable median/MAD over 50M), then the threshold is applied to all pixels.
    fn mark_bad_pixels(dark: &mut [u16], variance: Option<&[u16]>) {
        const N_SIGMA: f64 = 6.0; // outlier distance in MAD-sigmas; ~6 flags genuine hot pixels, spares normal noise
        let n = dark.len();
        // Robust threshold from a strided sample. stride chosen for ~300k samples regardless of resolution.
        let sample_thresh = |get: &dyn Fn(usize) -> u16| -> f64 {
            let target = 300_000usize.min(n.max(1));
            let stride = (n / target).max(1);
            let mut s: Vec<u16> = (0..n).step_by(stride).map(get).collect();
            s.sort_unstable();
            let med = s[s.len() / 2] as f64;
            // MAD = median(|x - med|); reuse the buffer for |deviation|.
            for v in s.iter_mut() {
                *v = (*v as f64 - med).abs() as u16;
            }
            s.sort_unstable();
            let mad = s[s.len() / 2] as f64;
            med + N_SIGMA * 1.4826 * mad.max(1.0) // mad.max(1): a degenerate all-equal sample would give MAD 0; floor it so the threshold stays finite
        };
        let lvl_thresh = sample_thresh(&|i| dark[i]);
        let var_thresh = variance.map(|v| sample_thresh(&|i| v[i]));
        for i in 0..n {
            let bad = dark[i] as f64 > lvl_thresh
                || var_thresh.map_or(false, |t| variance.unwrap()[i] as f64 > t);
            if bad {
                dark[i] = BAD;
            }
        }
    }

    /// Apply the correction to a light frame (u16, same w*h, the light's native raw scale). `iso_light` and
    /// `t_light_ns` are the light's capture ISO and effective exposure; `cal_scale` brings the cal's raw DN
    /// into the light's domain (white_light / white_cal, e.g. 64 for 10-bit-cal vs 16-bit-light). Writes the
    /// corrected frame in place semantics by returning a new Vec. Black pedestal is preserved.
    pub fn apply(&self, light: &[u16], iso_light: f64, t_light_ns: f64, cal_scale: f64) -> Vec<u16> {
        let n = self.width * self.height;
        // Not a defensive guard: the caller (nativeApplyCalToDng) already verified the DNG's raw strip is
        // exactly n pixels before calling, and indexes light[0..n] below. If light isn't n long, WE built it
        // wrong upstream - panic loud so the bug surfaces, don't silently emit a mis-corrected file.
        assert_eq!(light.len(), n, "calibration apply: light {} != {}x{}={}", light.len(), self.width, self.height, n);
        // Gain + exposure ratios. Guard the denominators (older cals may store 0 -> fall back to ratio 1).
        let g = if self.iso_cal > 0.0 { iso_light / self.iso_cal } else { 1.0 };
        let te = if self.exposure_ns > 0.0 { t_light_ns / self.exposure_ns } else { 1.0 };
        let black = self.black_level * cal_scale;

        // Dark/bias subtraction on every GOOD pixel. Bad pixels (dark == BAD) are left 0 here and filled by reconstruction below - subtracting their (noisy/blown) cal value can't recover them, so we replace from good neighbours instead.
        let mut out = vec![0u16; n];
        for i in 0..n {
            if self.dark[i] == BAD {
                continue; // reconstructed below
            }
            let bias = self.bias[i] as f64 * cal_scale;
            let dark = self.dark[i] as f64 * cal_scale;
            let offset_term = (bias - black) * g;
            let darkcurrent_term = (dark - bias) * g * te;
            let corrected = light[i] as f64 - offset_term - darkcurrent_term;
            // f64->u16 saturates: corrected < 0 -> 0 (clipped black), > 65535 -> 65535. round() for correct nearest-integer; no explicit clamp needed (the cast does it).
            out[i] = corrected.round() as u16;
        }

        // Reconstruct each bad pixel (dark == BAD) from the mean of its good SAME-CFA-PHASE neighbours - stride 4 (the 4x4 quad-Bayer period) lands on the same colour AND same sub-position, so we average like-with-like. Reads `out` (already-subtracted good pixels) so the fill matches the corrected scene; falls back to the subtracted value if a bad pixel has no good same-phase neighbour (vanishingly rare).
        let w = self.width;
        let h = self.height;
        let p = 4i64;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if self.dark[i] != BAD {
                    continue;
                }
                let mut s = 0.0f64;
                let mut c = 0.0f64;
                let mut acc = |xx: i64, yy: i64| {
                    if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                        let j = yy as usize * w + xx as usize;
                        if self.dark[j] != BAD {
                            s += out[j] as f64;
                            c += 1.0;
                        }
                    }
                };
                acc(x as i64 - p, y as i64);
                acc(x as i64 + p, y as i64);
                acc(x as i64, y as i64 - p);
                acc(x as i64, y as i64 + p);
                if c > 0.0 {
                    out[i] = (s / c).round() as u16;
                }
            }
        }
        out
    }
}
