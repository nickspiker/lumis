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
//! its normal BlackLevel tag and downstream demosaicers behave.
//!
//! NOTE: hot/dead-pixel masking + reconstruction is currently DISABLED - every pixel gets the same
//! dark/bias subtraction, nothing special-cased. This isolates the base correction so it can be verified
//! on its own; reconstruction can be layered back on once confirmed.
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
    pub dark: Vec<u16>, // per-pixel mean, max-ISO longest-shutter (bias + dark current)
}

impl LoadedCalibration {
    /// Combine a decoded BIAS file and a decoded DARK file into a ready calibration. Dimensions must match.
    /// Metadata (iso_cal/exposure_ns/black_level) comes from the DARK file (its exposure_ns = the 16s the
    /// dark current was integrated over).
    ///
    /// CONSUMES both CalFiles and MOVES their map Vecs (no clone). Each map is ~50M u16 = 100MB; on the
    /// camera process cloning them (plus the still-live raw VSF bytes) blew the memory budget and the OS
    /// silently killed the process. Moving keeps exactly one copy of bias + one of dark.
    pub fn from_pair(bias_file: CalFile, dark_file: CalFile) -> Option<LoadedCalibration> {
        if bias_file.width != dark_file.width || bias_file.height != dark_file.height {
            return None;
        }
        Some(LoadedCalibration {
            width: dark_file.width,
            height: dark_file.height,
            iso_cal: dark_file.iso_cal,
            exposure_ns: dark_file.exposure_ns,
            black_level: dark_file.black_level,
            bias: bias_file.mean, // moved, not cloned
            dark: dark_file.mean, // moved, not cloned
        })
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

        // Step 1: just the dark/bias subtraction on EVERY pixel - no hot-pixel skip, no reconstruction.
        // (Hot-pixel masking + reconstruction is deliberately disabled for now so we can verify the base
        // correction in isolation; it can be layered back on once this is confirmed.)
        let mut out = vec![0u16; n];
        for i in 0..n {
            let bias = self.bias[i] as f64 * cal_scale;
            let dark = self.dark[i] as f64 * cal_scale;
            let offset_term = (bias - black) * g;
            let darkcurrent_term = (dark - bias) * g * te;
            let corrected = light[i] as f64 - offset_term - darkcurrent_term;
            // f64->u16 saturates: corrected < 0 -> 0 (clipped black), > 65535 -> 65535. round() for correct
            // nearest-integer; no explicit clamp needed (the cast does it).
            out[i] = corrected.round() as u16;
        }
        out
    }
}
