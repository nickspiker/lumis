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
//! The flat black pedestal is deliberately LEFT in (only the per-pixel pattern is removed) so the DNG keeps
//! its normal BlackLevel tag and downstream demosaicers behave. Dead/hot pixels (flagged by the dark level
//! or its frame-to-frame variance) are RECONSTRUCTED from good same-CFA-phase neighbours instead of
//! subtracted, so a hot pixel neither leaves a hole nor injects garbage.
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

/// A decoded, ready-to-apply calibration (bias map + dark map + metadata + bad-pixel mask). Maps are raw
/// 10-bit DN (as stored); scaling to the light's domain happens in `apply`.
pub struct LoadedCalibration {
    pub width: usize,
    pub height: usize,
    pub iso_cal: f64,
    pub exposure_ns: f64,
    pub black_level: f64,
    pub bias: Vec<u16>, // per-pixel mean, max-ISO shortest-shutter
    pub dark: Vec<u16>, // per-pixel mean, max-ISO longest-shutter (bias + dark current)
    pub bad: Vec<bool>, // hot/unstable pixels (from dark level OR dark variance)
}

impl LoadedCalibration {
    /// Combine a decoded BIAS file and a decoded DARK file into a ready calibration. Dimensions must match.
    /// Metadata (iso_cal/exposure_ns/black_level) comes from the DARK file (its exposure_ns = the 16s the
    /// dark current was integrated over). The bad-pixel mask uses the DARK level + variance.
    ///
    /// CONSUMES both CalFiles and MOVES their map Vecs (no clone). Each map is ~50M u16 = 100MB; on the
    /// camera process cloning them (plus the still-live raw VSF bytes) blew the memory budget and the OS
    /// silently killed the process. Moving keeps exactly one copy of bias + one of dark.
    pub fn from_pair(bias_file: CalFile, dark_file: CalFile) -> Option<LoadedCalibration> {
        if bias_file.width != dark_file.width || bias_file.height != dark_file.height {
            return None;
        }
        let bad = match &dark_file.variance {
            Some(v) => Self::bad_mask(&dark_file.mean, v),
            None => dark_file.mean.iter().map(|&d| d > 140).collect(), // level-only fallback
        };
        Some(LoadedCalibration {
            width: dark_file.width,
            height: dark_file.height,
            iso_cal: dark_file.iso_cal,
            exposure_ns: dark_file.exposure_ns,
            black_level: dark_file.black_level,
            bias: bias_file.mean, // moved, not cloned
            dark: dark_file.mean, // moved, not cloned
            bad,
        })
    }

    /// Hot/unstable pixel mask: bad if the dark adder is too hot (level) OR its frame-to-frame diff is too
    /// unstable (variance). Thresholds are well above the dark's bulk; tuned in the host harness.
    fn bad_mask(dark: &[u16], variance: &[u16]) -> Vec<bool> {
        // Raw 10-bit DN thresholds (the harness used 16-bit /64): level ~140 DN (=9000/64), diff ~125 (=8000/64).
        const HOT_LEVEL: u16 = 140;
        const HOT_DIFF: u16 = 125;
        (0..dark.len()).map(|i| dark[i] > HOT_LEVEL || variance[i] > HOT_DIFF).collect()
    }

    /// Apply the correction to a light frame (u16, same w*h, the light's native raw scale). `iso_light` and
    /// `t_light_ns` are the light's capture ISO and effective exposure; `cal_scale` brings the cal's raw DN
    /// into the light's domain (white_light / white_cal, e.g. 64 for 10-bit-cal vs 16-bit-light). Writes the
    /// corrected frame in place semantics by returning a new Vec. Black pedestal is preserved.
    pub fn apply(&self, light: &[u16], iso_light: f64, t_light_ns: f64, cal_scale: f64) -> Vec<u16> {
        let n = self.width * self.height;
        if light.len() != n {
            return light.to_vec(); // size mismatch: pass through unchanged
        }
        // Gain + exposure ratios. Guard the denominators (older cals may store 0 -> fall back to ratio 1).
        let g = if self.iso_cal > 0.0 { iso_light / self.iso_cal } else { 1.0 };
        let te = if self.exposure_ns > 0.0 { t_light_ns / self.exposure_ns } else { 1.0 };
        let black = self.black_level * cal_scale;

        let mut out = vec![0u16; n];
        for i in 0..n {
            if self.bad[i] {
                continue; // fill after, by reconstruction
            }
            let bias = self.bias[i] as f64 * cal_scale;
            let dark = self.dark[i] as f64 * cal_scale;
            let offset_term = (bias - black) * g;
            let darkcurrent_term = (dark - bias) * g * te;
            let corrected = light[i] as f64 - offset_term - darkcurrent_term;
            out[i] = corrected.clamp(0.0, 65535.0).round() as u16;
        }
        // Reconstruct bad pixels from good same-CFA-phase neighbours (stride 4 for the 4x4 quad CFA; lands on
        // the same colour). Reads `out` (already-corrected good pixels) so the fill matches the corrected scene.
        let w = self.width;
        let h = self.height;
        let period = 4i64;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if !self.bad[i] {
                    continue;
                }
                let mut s = 0.0f64;
                let mut c = 0.0f64;
                let mut acc = |xx: i64, yy: i64| {
                    if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                        let j = yy as usize * w + xx as usize;
                        if !self.bad[j] {
                            s += out[j] as f64;
                            c += 1.0;
                        }
                    }
                };
                acc(x as i64 - period, y as i64);
                acc(x as i64 + period, y as i64);
                acc(x as i64, y as i64 - period);
                acc(x as i64, y as i64 + period);
                out[i] = if c > 0.0 {
                    (s / c).clamp(0.0, 65535.0).round() as u16
                } else {
                    light[i] // no good neighbour: leave the original (vanishingly rare)
                };
            }
        }
        out
    }
}
