use crate::image::dng::*;
use crate::shared_memory::*;
use chrono::{DateTime, Local};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::{thread, u32};

// Global QUEUE of pending save data (bytes + filename). A queue (not a single slot) so one capture can emit
// multiple files - e.g. an uncorrected DNG plus a calibration-corrected DNG. Kotlin drains one per poll.
static PENDING_SAVE_DATA: Mutex<std::collections::VecDeque<(Vec<u8>, String)>> =
    Mutex::new(std::collections::VecDeque::new());

/// Pop the next pending save (FIFO) for JNI save operations. None when the queue is empty.
pub fn get_pending_save_data() -> Option<(Vec<u8>, String)> {
    PENDING_SAVE_DATA.lock().unwrap().pop_front()
}

/// Enqueue one pending save (FIFO).
pub fn set_pending_save_data(dng_data: Vec<u8>, filename: String) {
    PENDING_SAVE_DATA.lock().unwrap().push_back((dng_data, filename));
}

/// Read an entire file referenced by a raw fd into a Vec (native memory). dup()s so our File owns its own
/// descriptor; reads from the start (rewinds via the dup). Used by the save thread to stream the cal VSFs
/// without holding them resident. None on any error. The caller still owns/closes the original fd.
fn read_fd_bytes(fd: i32) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::fd::FromRawFd;
    let dup = unsafe { libc::dup(fd) };
    if dup < 0 {
        return None;
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(dup) };
    let _ = file.seek(SeekFrom::Start(0)); // a prior save may have read to EOF on the shared description
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?; // File drop closes `dup`
    Some(buf)
}

pub struct CameraSettings {
    pub iso: f32,
    pub shutter_ns: f32,
    pub focus_distance: f32,
}

pub struct IntegrationBuffer {
    pub accumulated: Vec<u32>,
    pub difference: Vec<u32>,
    pub prev_frame: Vec<u16>,
}

#[derive(Clone)]
pub enum TimeBase {
    Minute,
    Hour,
    Day,
    Month,
    Year,
}

impl TimeBase {
    pub fn next(self) -> Self {
        match self {
            TimeBase::Minute => TimeBase::Hour,
            TimeBase::Hour => TimeBase::Day,
            TimeBase::Day => TimeBase::Month,
            TimeBase::Month => TimeBase::Year,
            TimeBase::Year => TimeBase::Minute,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TimeBase::Minute => "MINUTE",
            TimeBase::Hour => "HOUR",
            TimeBase::Day => "DAY",
            TimeBase::Month => "MONTH",
            TimeBase::Year => "YEAR",
        }
    }

    pub fn duration_ms(&self) -> f64 {
        match self {
            TimeBase::Minute => 1000. * 60.,
            TimeBase::Hour => 1000. * 60. * 60.,
            TimeBase::Day => 1000. * 60. * 60. * 24.,
            TimeBase::Month => 1000. * 60. * 60. * 24. * 28.,
            TimeBase::Year => 1000. * 60. * 60. * 24. * (28. * 13. + 1.),
        }
    }
}

/// Unpack Android RAW10 (MIPI CSI-2 packing) into 2-bytes-per-pixel little-endian u16 values (0..=1023). RAW10 stores 4 pixels in 5 bytes: 4 high bytes then a 5th byte holding the 2 low bits of each, and each row is padded to `row_stride` bytes. Output is tightly packed width*height*2 bytes (no row padding), matching the layout the integrator's loops read.
fn unpack_raw10(packed: &[u8], width: usize, height: usize, row_stride: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 2];
    for y in 0..height {
        let row = &packed[y * row_stride..];
        let out_row = y * width * 2;
        let groups = width / 4;
        for g in 0..groups {
            let p = g * 5;
            // Guard against a short final group / truncated buffer.
            if p + 5 > row.len() {
                break;
            }
            let lo = row[p + 4];
            // pixel = (high_byte << 2) | (its 2 low bits). Values are 0..=1023.
            let px = [
                ((row[p] as u16) << 2) | ((lo & 0x03) as u16),
                ((row[p + 1] as u16) << 2) | (((lo >> 2) & 0x03) as u16),
                ((row[p + 2] as u16) << 2) | (((lo >> 4) & 0x03) as u16),
                ((row[p + 3] as u16) << 2) | (((lo >> 6) & 0x03) as u16),
            ];
            let o = out_row + g * 8;
            for (k, &v) in px.iter().enumerate() {
                let b = v.to_le_bytes();
                out[o + k * 2] = b[0];
                out[o + k * 2 + 1] = b[1];
            }
        }
    }
    out
}

/// Pearson correlation between two equal-length slices. Used for the dark-frame calibration's
/// split-half convergence metric (even-frame averages vs odd-frame averages over the pixel sample):
/// ~1.0 means the two independent halves agree, i.e. the fixed pattern has emerged from the noise.
fn pearson(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    if n < 2.0 {
        return 0.0;
    }
    let ma = a.iter().sum::<f64>() / n;
    let mb = b.iter().sum::<f64>() / n;
    let mut cov = 0.0;
    let mut va = 0.0;
    let mut vb = 0.0;
    for k in 0..a.len() {
        let da = a[k] - ma;
        let db = b[k] - mb;
        cov += da * db;
        va += da * da;
        vb += db * db;
    }
    let denom = (va * vb).sqrt();
    if denom > 0.0 {
        cov / denom
    } else {
        0.0
    }
}

pub struct CameraIntegrator {
    shared_memory: SharedMemory, // Keep SharedMemory alive
    pub header: &'static mut [u64],
    image_buffer: &'static mut [u16],
    // Slitscan ring (width x 2*width), in its own segment region after image_buffer. Written per-frame in
    // RawMode::Slitscan; never touched by the slot/accumulate path, so a held stack survives a trip into the
    // strip and back. `was_slitscan` tracks the previous frame's mode so the ring is reset once on entry.
    slitscan_buffer: &'static mut [u16],
    was_slitscan: bool,
    // u32 accumulator for the live slitscan column (period rows x width): each column integrates frames over
    // the set exposure time exactly like a normal Average exposure, then averages + scales into the ring and
    // advances the head. Keeps slitscan's frame_count / effective-ISO metadata identical to the other modes.
    slitscan_accum: Vec<u32>,
    // Count of completed full ring rotations (the head wrapping back to 0). Continuous-save in slitscan fires
    // once per rotation off this, instead of once per column like the per-column image counter.
    slitscan_rotations: u64,
    // Whether slitscan continuous-save was active last poll, to detect its activation edge and arm the first
    // save to the next full rotation (so enabling it mid-fill doesn't dump a half-filled strip).
    prev_slit_continuous: bool,
    pub magic_9_display: &'static mut [f32; 9],
    pub magic_9_display_gamma: &'static mut f32,
    pub magic_9_dng_xyz: &'static mut [f32; 9],
    pub magic_9_dng_xyz_gamma: &'static mut f32,
    integration_buffer: IntegrationBuffer,
    pub width: usize,
    pub height: usize,
    white_level: u16,
    black_level: u16,
    exposure_time_ms: u64,
    exposure_start_time: Instant,
    last_frame_time: Instant,
    last_image_timestamp: SystemTime,
    last_saved_counter: u64,
    // Dark-frame calibration state (only used while CALIBRATING_BIT is set). None until the first
    // calibration frame arrives; reset when finalized so a later calibration starts fresh.
    cal: Option<CalibrationState>,
    // Dup'd fds for the loaded calibration's bias + dark .vsf files (set by Kotlin at camera open when a cal exists for this sensor/resolution; -1 = none). The save thread re-reads + decodes these per save (transient, freed after - NOT held resident, which OOM-killed the camera process), applies the correction to the raw before the format branch, so every output format is calibrated.
    cal_bias_fd: i32,
    cal_dark_fd: i32,
    cal_white_level: u16, // sensor white level captured at load, for the cal->light scale (65535/white)
}

// Accumulated state for a running dark-frame calibration capture. Mean and variance use the shared
// IntegrationBuffer (accumulated / difference); this holds the extra state: the running frame count,
// timers, and the SAMPLE-ONLY even/odd half-sums for the live split-half convergence correlation
// (full even/odd frames would be ~400MB at 50MP, so we correlate a fixed random ~300k-pixel subset).
struct CalibrationState {
    frame_count: u64,
    start_time: Instant,
    last_snapshot: Instant,
    last_accept: Instant,   // arrival of the last ACCEPTED dark frame; gates the next on inter-frame time
    sample_idx: Vec<usize>, // fixed random pixel indices used for the correlation sample
    even_sum: Vec<u64>,     // per-sample sum over even-numbered frames
    odd_sum: Vec<u64>,      // per-sample sum over odd-numbered frames
    // The ISO and shutter the HAL ACTUALLY applied on accepted frames (last accepted wins - post-settle and
    // stable). Stored in the cal VSF so apply-time knows the gain/exposure this cal was shot at, WITHOUT
    // re-querying the sensor's max ISO (a firmware/HAL update can move the ISO range, which would corrupt
    // the gain ratio if we trusted "current max" instead of the concrete value the cal was captured at).
    captured_iso: f64,
    captured_shutter_ns: f64,
}

impl CameraIntegrator {
    pub fn new(
        width: usize,
        height: usize,
        white_level: u16,
        black_level: u16,
        bayer_pattern: u32,
        camera_facing: u32,
        sensor_orientation: i32,
        shared_memory_size: usize,
        min_iso: f64,
        max_iso: f64,
        min_exposure_ns: f64,
        max_exposure_ns: f64,
        min_focus: f64,
        initial_iso: f64,
        initial_shutter_ns: f64,
    ) -> Self {
        let pixel_count = width * height;

        // Use ASharedMemory on Android, regular allocation otherwise
        #[cfg(target_os = "android")]
        let mut shared_memory = SharedMemory::create_ashmem(shared_memory_size);

        #[cfg(not(target_os = "android"))]
        let mut shared_memory = SharedMemory::create(shared_memory_size);

        // Get header and image slices with static lifetime
        let header = unsafe {
            std::mem::transmute::<&mut [u64], &'static mut [u64]>(shared_memory.as_slice())
        };
        let image_buffer = unsafe {
            std::mem::transmute::<&mut [u16], &'static mut [u16]>(
                shared_memory.image_buffer(pixel_count),
            )
        };
        let slitscan_buffer = unsafe {
            std::mem::transmute::<&mut [u16], &'static mut [u16]>(
                shared_memory.slitscan_buffer(pixel_count, width),
            )
        };

        // Initialize shared memory header
        header[IMAGE_COUNTER_IDX] = 0;
        header[SENSOR_WIDTH_IDX] = width as u64;
        header[SENSOR_HEIGHT_IDX] = height as u64;
        header[SENSOR_BAYER_PATTERN_IDX] = bayer_pattern as u64;
        header[WHITE_LEVEL_IDX] = white_level as u64;
        header[BLACK_LEVEL_IDX] = black_level as u64;
        header[MIN_ISO_IDX] = min_iso.to_bits();
        header[MAX_ISO_IDX] = max_iso.to_bits();
        header[SHORTEST_SHUTTER_NS_IDX] = min_exposure_ns.to_bits();
        header[LONGEST_SHUTTER_NS_IDX] = max_exposure_ns.to_bits();
        header[MIN_FOCUS_IDX] = min_focus.to_bits();
        header[CAMERA_FACING_IDX] = camera_facing as u64;
        header[SENSOR_ORIENTATION_IDX] = sensor_orientation as u64;
        header[SAVED_COUNTER_IDX] = 0;
        header[CURRENT_MODE_IDX] = 0;
        header[FRAME_COUNTER_IDX] = 0;
        header[EXPOSURE_START_SECS_IDX] = 0;
        header[EXPOSURE_START_NANOS_IDX] = 0;
        header[FPS_IDX] = 0f64.to_bits();

        // Seed ISO and per-frame shutter from the one-shot auto-exposure result that Kotlin metered before creating this integrator, clamped to the sensor's range. This makes the opening preview usable on any device/lighting without hardcoding a value (sensor ranges vary wildly: min ISO 16 here could be 1 elsewhere, and max exposure ≈16s here could be a full day). A bad seed previously defaulted shutter to max_exposure_ns, so the feed delivered one frame every ~16s and looked frozen. Fall back to min when the AE value is missing (<= 0).
        let seed_iso = if initial_iso > 0.0 {
            initial_iso.clamp(min_iso, max_iso)
        } else {
            min_iso
        };
        let seed_shutter = if initial_shutter_ns > 0.0 {
            initial_shutter_ns.clamp(min_exposure_ns, max_exposure_ns)
        } else {
            min_exposure_ns
        };
        header[ISO_IDX] = seed_iso.to_bits();
        header[SHUTTER_NS_IDX] = seed_shutter.to_bits();
        header[FOCUS_IDX] = 0f64.to_bits();
        header[EXPOSURE_TIME_MS_IDX] = 0f64.to_bits();
        let unix_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        header[HEARTBEAT_SECS_IDX] = unix_time.as_secs();
        header[HEARTBEAT_NANOS_IDX] = unix_time.subsec_nanos() as u64;
        header[FLAGS_IDX] = 0;

        // for pixel in image_buffer {
        //     *pixel = 0;
        // }

        let integration_buffer = IntegrationBuffer {
            accumulated: vec![u32::MAX; pixel_count],
            difference: vec![0u32; pixel_count],
            prev_frame: vec![0u16; pixel_count],
        };

        let (magic_9_display, magic_9_display_gamma) = shared_memory.get_magic_9_display_slices();
        let (magic_9_dng_xyz, magic_9_dng_xyz_gamma) = shared_memory.get_magic_9_dng_xyz_slices();

        // Initialize magic_9_display to identity matrix if not already set
        if magic_9_display[0] == 0.0 && magic_9_display[4] == 0.0 && magic_9_display[8] == 0.0 {
            magic_9_display[0] = 1.0;
            magic_9_display[4] = 1.0;
            magic_9_display[8] = 1.0;
            *magic_9_display_gamma = 2.2;
            log::info!("Integrator: Initialized magic_9_display to identity matrix");
        }

        Self {
            shared_memory,
            header,
            image_buffer,
            slitscan_buffer,
            was_slitscan: false,
            slitscan_accum: Vec::new(),
            slitscan_rotations: 0,
            prev_slit_continuous: false,
            magic_9_display,
            magic_9_display_gamma,
            magic_9_dng_xyz,
            magic_9_dng_xyz_gamma,
            integration_buffer,
            width,
            height,
            white_level,
            black_level,
            exposure_time_ms: 0,
            exposure_start_time: Instant::now(),
            last_frame_time: Instant::now(),
            last_image_timestamp: SystemTime::now(),
            last_saved_counter: u64::MAX,
            cal: None,
            cal_bias_fd: -1,
            cal_dark_fd: -1,
            cal_white_level: 0,
        }
    }

    /// Install (or clear) the loaded-calibration fds. Kotlin dup()s the bias + dark .vsf file descriptors at
    /// camera open and passes them here; the save thread re-reads them per save. Pass bias_fd<0 or dark_fd<0
    /// to clear (no calibration -> saves are uncorrected). white_level is the sensor white for the cal scale.
    pub fn set_calibration_fds(&mut self, bias_fd: i32, dark_fd: i32, white_level: u16) {
        // Close any previously-held fds before replacing, so re-opening the camera doesn't leak descriptors.
        if self.cal_bias_fd >= 0 {
            unsafe { libc::close(self.cal_bias_fd); }
        }
        if self.cal_dark_fd >= 0 {
            unsafe { libc::close(self.cal_dark_fd); }
        }
        self.cal_bias_fd = bias_fd;
        self.cal_dark_fd = dark_fd;
        self.cal_white_level = white_level;
    }

    pub fn process_frame(
        &mut self,
        frame_data_in: &[u8],
        _captured_iso: i32,
        _captured_shutter_ns: i64,
        _captured_focus: f32,
        // RAW10 path: when true, frame_data_in is MIPI-packed RAW10 (4 px / 5 bytes, rows padded to row_stride bytes). We depack it once into 16-bit LE here so the rest of the pipeline (which reads 2 bytes/pixel) is unchanged. row_stride is in BYTES.
        raw10: bool,
        row_stride: usize,
    ) -> (i32, i64, f32) {
        // Tell the UI process whether this frame is quad-Bayer (max-res RAW10 = a Tetracell 4x4 CFA). The UI's calibration path needs this to pre-bin before handing the frame to chameleon (whose debayer only understands standard 2x2 Bayer).
        self.header[QUAD_BAYER_IDX] = if raw10 { 1 } else { 0 };
        // Depack RAW10 -> the same 2-bytes-per-pixel little-endian layout the integrator's accumulation loops expect (value 0..=1023, since white_level is 10-bit). Done once per frame on the camera thread; pixels are read many times downstream.
        let unpacked: Vec<u8>;
        let frame_data: &[u8] = if raw10 {
            unpacked = unpack_raw10(frame_data_in, self.width, self.height, row_stride);
            &unpacked
        } else {
            frame_data_in
        };
        // Calculate FPS based on frame timing
        let now = Instant::now();
        let frame_duration = now.duration_since(self.last_frame_time).as_secs_f64();
        let fps = 1. / frame_duration;
        self.header[FPS_IDX] = fps.to_bits();

        self.last_frame_time = now;

        // Check UI heartbeat - nuke if no activity for >1.6 seconds
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let heartbeat_secs = self.header[HEARTBEAT_SECS_IDX];
        let heartbeat_nanos = self.header[HEARTBEAT_NANOS_IDX];

        let heartbeat_time = std::time::Duration::new(heartbeat_secs, heartbeat_nanos as u32);
        let elapsed_since_heartbeat = current_time - heartbeat_time;

        if elapsed_since_heartbeat.as_millis() > 1623
            && (self.header[FLAGS_IDX] & CONTINUOUS_SAVE_BIT) == 0
        {
            if crate::DEBUG {
                log::info!(
                    "UI heartbeat timeout: {}ms elapsed - camera thread auto-nuke activated",
                    elapsed_since_heartbeat.as_millis()
                );
            }
            unsafe {
                libc::exit(0);
            }
        }

        // While the finalized calibration result is on screen (CAL_SHOW_RESULT_BIT, set after finalize
        // until the user taps), freeze: do NOT process incoming frames. The result view displays the
        // averaged frame in image_buffer slot 0; the normal accumulate path below would overwrite those
        // slots with each new frame - harmless at 16s darks but at fast BIAS frames it scribbles over the
        // preview every few ms ("the preview continuously changes") and races the UI's read. Return the
        // current settings (heartbeat already updated above, so the process stays alive) and skip the rest.
        if (self.header[FLAGS_IDX] & CAL_SHOW_RESULT_BIT) != 0 {
            return (
                f64::from_bits(self.header[ISO_IDX]) as i32,
                f64::from_bits(self.header[SHUTTER_NS_IDX]) as i64,
                f64::from_bits(self.header[FOCUS_IDX]) as f32,
            );
        }

        self.exposure_time_ms = f64::from_bits(self.header[EXPOSURE_TIME_MS_IDX]) as u64;

        // Check for Bluetooth shutter signal (force completion)
        let force_completion = (self.header[FLAGS_IDX] & COMPLETE_EXPOSURE_BIT) != 0;
        self.header[FLAGS_IDX] &= !COMPLETE_EXPOSURE_BIT;

        let elapsed_ms = self.exposure_start_time.elapsed().as_millis() as u64;
        self.header[FRAME_COUNTER_IDX] += 1;

        // Write exposure start time to SharedMemory for UI progress tracking
        // Convert Instant to SystemTime by calculating offset from now
        let now_instant = std::time::Instant::now();
        let now_system = std::time::SystemTime::now();
        let elapsed_since_start = now_instant.duration_since(self.exposure_start_time);
        let start_system_time = now_system - elapsed_since_start;
        let start_time = start_system_time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        self.header[EXPOSURE_START_SECS_IDX] = start_time.as_secs();
        self.header[EXPOSURE_START_NANOS_IDX] = start_time.subsec_nanos() as u64;

        // Dark-frame calibration: accumulate without the normal per-exposure reset/completion, write
        // throttled progress stats, and finalize-to-disk on request. Handled entirely here so the
        // verified normal exposure path below is never touched while calibrating.
        if (self.header[FLAGS_IDX] & CALIBRATING_BIT) != 0 {
            self.process_calibration_frame(frame_data, _captured_iso, _captured_shutter_ns);
            // Return the header's current (forced) settings so Kotlin keeps pushing them to the HAL,
            // same as the normal path's tail return.
            return (
                f64::from_bits(self.header[ISO_IDX]) as i32,
                f64::from_bits(self.header[SHUTTER_NS_IDX]) as i64,
                f64::from_bits(self.header[FOCUS_IDX]) as f32,
            );
        }

        // SLITSCAN: a photo-finish time-strip. Instead of accumulating whole frames, take a thin horizontal
        // slice (one full Bayer period of rows, so the strip stays a valid demosaicable raw) from the centre
        // of EACH frame and write it into a ring carved from the shared image_buffer; time fills the vertical
        // axis. The ring is width x (2*width) -> a 2:1 strip; the write-head advances per frame and wraps,
        // overwriting the oldest slice with NO data shuffling (same idea as the rolling slots). Handled
        // entirely here; the normal accumulate path below is untouched.
        if self.header[CURRENT_MODE_IDX] as u8 == RawMode::Slitscan as u8 {
            self.process_slitscan_frame(frame_data, elapsed_ms, force_completion);
            return (
                f64::from_bits(self.header[ISO_IDX]) as i32,
                f64::from_bits(self.header[SHUTTER_NS_IDX]) as i64,
                f64::from_bits(self.header[FOCUS_IDX]) as f32,
            );
        }
        // Past here is a non-slitscan frame: clear the entry latch so re-entering slitscan zeroes the ring.
        self.was_slitscan = false;

        if elapsed_ms >= self.exposure_time_ms || force_completion {
            // Exposure complete - write to SharedMemory
            let pixel_count = self.width * self.height;
            let new_image_count = self.header[IMAGE_COUNTER_IDX] + 1;
            let current_slot = ((new_image_count) & 3) as usize;
            let frame_count = self.header[FRAME_COUNTER_IDX];

            let chunks = frame_data.chunks_exact(2);
            for (i, chunk) in chunks.enumerate() {
                let pixel = u16::from_le_bytes([chunk[0], chunk[1]]);

                // Calculate indices for frame interleaved rolling buffer (ADADADAD)
                let avg_idx = i + (current_slot * 2) * pixel_count; // Frame N avg
                let diff_idx = i + (current_slot * 2 + 1) * pixel_count; // Frame N diff

                // Write accumulated/difference to rolling buffer
                self.image_buffer[avg_idx] = (((self.integration_buffer.accumulated[i] as u64)
                    << 16)
                    / (self.white_level as u64 * frame_count))
                    .min(65535) as u16;
                self.image_buffer[diff_idx] = (((self.integration_buffer.difference[i] as u64)
                    << 16)
                    / ((self.white_level as u64 - self.black_level as u64) * frame_count))
                    .min(65535) as u16;

                // Reset integration buffer for next exposure
                self.integration_buffer.accumulated[i] = pixel as u32;
                self.integration_buffer.difference[i] =
                    (pixel as i32 - self.integration_buffer.prev_frame[i] as i32).abs() as u32;
                self.integration_buffer.prev_frame[i] = pixel;
            }

            self.exposure_start_time = Instant::now();

            self.header[IMAGE_COUNTER_IDX] = new_image_count;
            self.header[FRAME_COUNTER_IDX] = 0;

            self.last_image_timestamp = SystemTime::now()
        } else {
            // Accumulate frame data
            let chunks = frame_data.chunks_exact(2);
            for (i, chunk) in chunks.enumerate() {
                let pixel = u16::from_le_bytes([chunk[0], chunk[1]]);

                self.integration_buffer.accumulated[i] += pixel as u32;

                let diff =
                    (pixel as i32 - self.integration_buffer.prev_frame[i] as i32).abs() as u32;
                self.integration_buffer.difference[i] += diff;
                self.integration_buffer.prev_frame[i] = pixel;
            }
        }

        // Check for save requests; the heavy save runs in try_save (also reachable from the
        // settings poll so a save fires immediately, not only when the next 16s frame arrives).
        self.try_save();
        // Send Kotlin user settings from header
        let current_iso = f64::from_bits(self.header[ISO_IDX]);
        let current_shutter_ns = f64::from_bits(self.header[SHUTTER_NS_IDX]);
        let current_focus = f64::from_bits(self.header[FOCUS_IDX]);
        (
            current_iso as i32,
            current_shutter_ns as i64,
            current_focus as f32,
        )
    }


    /// Check the save flags and, if a manual/continuous save is pending and not already saving,
    /// spawn the encode+write from the ALREADY-PUBLISHED image_buffer slot. Callable from
    /// process_frame and from the 30Hz settings poll (nativeCheckSave) so a save at a long
    /// exposure fires within ~33ms instead of waiting for the next frame - the data already
    /// exists in image_buffer; only the flag check was frame-gated.
    pub fn try_save(&mut self) {
        // Quad-Bayer-ness comes from the header here (process_frame's raw10 param isn't in scope).
        let raw10 = self.header[QUAD_BAYER_IDX] != 0;
        let flags = self.header[FLAGS_IDX];
        let manual_save = (flags & MANUAL_SAVE_BIT) != 0;
        let continuous_save = (flags & CONTINUOUS_SAVE_BIT) != 0;
        let already_saving = (flags & CURRENTLY_SAVING) != 0;

        // Continuous-save cadence is "a new image". In slitscan IMAGE_COUNTER ticks every COLUMN (so the
        // preview scrolls), which would fire a save every column; gate slitscan continuous-save on the
        // full-rotation counter instead, so it saves one complete strip per ring rotation.
        let is_slitscan = self.header[CURRENT_MODE_IDX] as u8 == RawMode::Slitscan as u8;
        let trigger_counter = if is_slitscan {
            self.slitscan_rotations
        } else {
            self.header[IMAGE_COUNTER_IDX]
        };
        // When slitscan continuous-save first becomes active (toggled on, or entered with it already on), arm
        // to the CURRENT rotation so the first save waits for the NEXT full ring rotation - enabling it partway
        // through a fill never dumps a half-filled strip. Manual (single) save ignores this and saves straight
        // away below.
        let slit_continuous = is_slitscan && continuous_save;
        if slit_continuous && !self.prev_slit_continuous {
            self.last_saved_counter = trigger_counter;
        }
        self.prev_slit_continuous = slit_continuous;

        if !already_saving
            && ((continuous_save && (self.last_saved_counter != trigger_counter)) || manual_save)
        {
            // Set the CURRENTLY_SAVING flag immediately to prevent duplicate saves
            self.header[FLAGS_IDX] |= CURRENTLY_SAVING;
            self.last_saved_counter = trigger_counter;
            log::info!("Save requested");
            let current_slot = (self.header[IMAGE_COUNTER_IDX] & 3) as usize;
            let current_mode = self.header[CURRENT_MODE_IDX] as u8;
            let sensor_black_level = self.header[BLACK_LEVEL_IDX] as u16;
            let sensor_white_level = self.header[WHITE_LEVEL_IDX] as u16;
            let bayer_pattern = self.header[SENSOR_BAYER_PATTERN_IDX] as u32;
            // Correct display rotation = SENSOR MOUNT angle + how the phone is HELD, both in degrees.
            // The sensor reads out in its mounted orientation (typically 90deg = sideways), so even a
            // "normal" portrait shot needs the mount angle to look upright; device_rotation (gravity) then
            // accounts for how it's held. Summing mod 360 is the standard Camera2 orientation formula.
            // (The old code used only the mount angle AND mis-read its degrees as a 0/1/2/3 index -> nothing
            // ever rotated; then I briefly used only device_rotation -> dropped the mount term, so portrait
            // came out sideways. Both terms are needed.)
            let sensor_mount = self.header[SENSOR_ORIENTATION_IDX] as u16;
            let device_held = self.header[DEVICE_ROTATION_IDX] as u16;
            let device_orientation = (sensor_mount + device_held) % 360;
            let pixel_count = self.width * self.height;
            // Slitscan saves the whole ring (width x ring_rows, a 2:1 time-strip), not a single slot. Its
            // dimensions differ from the per-frame width/height, so resolve the save dimensions once here
            // and use them for the EXIF, the raw buffer and the encoders below. Non-slitscan modes save the
            // native frame.
            let is_slitscan = current_mode == 3;
            let (save_width, save_height) = if is_slitscan {
                let rr = self.slitscan_buffer.len() / self.width.max(1);
                (self.width, rr)
            } else {
                (self.width, self.height)
            };
            // JXL fallback: if the format is JXL but this device's MediaStore can't accept it (flag = 2),
            // save as JPEG instead. Belt-and-suspenders for the zero-init default (JXL=0); the UI cycle
            // also skips JXL when unsupported, but the very first save could still land on the default.
            let save_format = {
                let f = self.header[SAVE_FORMAT_IDX];
                if f == SAVE_FORMAT_JPEGXL && self.header[JXL_SUPPORTED_IDX] == 2 {
                    SAVE_FORMAT_JPEG
                } else {
                    f
                }
            };
            // Exposure provenance for ImageDescription: per-frame shutter + ISO that the HAL set, the
            // number of integrated frames, and the composite/effective exposure. Summing/averaging N
            // frames gathers N x the light, so the EFFECTIVE integration time is N x shutter and the
            // effective ISO drops by N (the gain you no longer need). frame_count resets per exposure,
            // so .max(1) guards a mid-first-frame save.
            let frame_count = self.header[FRAME_COUNTER_IDX].max(1);
            let per_frame_shutter_ns = f64::from_bits(self.header[SHUTTER_NS_IDX]);
            let per_frame_iso = f64::from_bits(self.header[ISO_IDX]);
            let integ_s = per_frame_shutter_ns * frame_count as f64 / 1.0e9;
            let eff_iso = per_frame_iso / frame_count as f64;
            // User display gain (0/unset -> 1.0). Baked into RGB exports and written as DNG
            // BaselineExposure (log2(gain) stops) so the saved file matches the on-screen brightness.
            let display_gain = {
                let g = f64::from_bits(self.header[DISPLAY_GAIN_IDX]);
                if g > 0.0 { g } else { 1.0 }
            };
            let baseline_exposure_stops = display_gain.log2();
            // Capture mode (the integration type) - recorded in the description so the saved file knows
            // whether it's an average, a frame-to-frame difference, or a motion (diff/avg) image.
            let mode_name = match current_mode {
                0 => "average",
                1 => "difference",
                2 => "motion",
                3 => "slitscan",
                _ => "unknown",
            };
            let exposure_desc = format!(
                "Lumis {} | integration: {}x {:.4}s @ ISO {:.0} | effective {:.3}s @ ISO {:.1}",
                mode_name,
                frame_count,
                per_frame_shutter_ns / 1.0e9,
                per_frame_iso,
                integ_s,
                eff_iso
            );
            // Structured EXIF for the EXIF IFD (DNG/JPEG/TIFF). Uses the COMPOSITE/effective exposure
            // (integ_s, eff_iso) since that's what the saved image physically represents. Focal length,
            // aperture and active-array diagonal come from the header (lens metadata, 0 = unknown);
            // focus distance is FOCUS_IDX in DIOPTERS (1/m), converted to metres for SubjectDistance.
            let focal_mm = f64::from_bits(self.header[FOCAL_LENGTH_MM_IDX]);
            let aperture_fnum = f64::from_bits(self.header[APERTURE_FNUM_IDX]);
            let sensor_diag_mm = f64::from_bits(self.header[SENSOR_DIAG_MM_IDX]);
            let focus_diopters = f64::from_bits(self.header[FOCUS_IDX]);
            let focus_distance_m = if focus_diopters > 0.0 { 1.0 / focus_diopters } else { 0.0 };
            // 35mm-equivalent focal = focal * (43.27 / sensor_diagonal_mm) (43.27 = full-frame diagonal).
            let focal_35mm = if sensor_diag_mm > 0.0 { focal_mm * 43.27 / sensor_diag_mm } else { 0.0 };
            let capture_dt: DateTime<Local> = DateTime::<Local>::from(self.last_image_timestamp);
            let exif = crate::image::dng::ExifData {
                exposure_time_s: integ_s,
                iso: eff_iso,
                f_number: aperture_fnum,
                focal_length_mm: focal_mm,
                focal_length_35mm: focal_35mm,
                subject_distance_m: focus_distance_m,
                datetime_original: capture_dt.format("%Y:%m:%d %H:%M:%S").to_string(),
                has_gps: self.header[GPS_HAS_FIX_IDX] != 0,
                gps_lat: f64::from_bits(self.header[GPS_LAT_IDX]),
                gps_lon: f64::from_bits(self.header[GPS_LON_IDX]),
                gps_alt: f64::from_bits(self.header[GPS_ALT_IDX]),
                image_width: save_width as u32,
                image_height: save_height as u32,
                // TIFF/EXIF Orientation for the JPEG/TIFF/JXL writers (the DNG sets its own from RawInfo).
                // Device rotation degrees -> EXIF value: 90->6 (90 CW), 180->3, 270->8 (90 CCW), else normal.
                // The exported samples stay in native sensor orientation; this tag is how viewers rotate them.
                orientation: match device_orientation {
                    90 => 6,
                    180 => 3,
                    270 => 8,
                    _ => 1,
                },
            };
            // XYZ matrix for RGB exports, and the magic9inv bytes for the DNG ColorMatrix1. magic_9_dng_xyz lives in zero-initialized shared memory and is only populated by a calibration scan. Pre-calibration it is all zeros, which would multiply every exported pixel to black; fall back to identity so uncalibrated RGB exports show the raw debayered scene (accuracy doesn't matter until calibrated anyway).
            let xyz_matrix = {
                let m = *self.magic_9_dng_xyz;
                if m.iter().all(|&v| v == 0.) {
                    [1., 0., 0., 0., 1., 0., 0., 0., 1.]
                } else {
                    m
                }
            };
            // RGB exports (JPEG/TIFF/JXL) are tagged Rec.2020 to match the on-screen preview, so they use the camera->Rec.2020 display matrix, NOT the DNG's XYZ matrix. Same zero-fallback to identity for the uncalibrated case.
            let display_matrix = {
                let m =
                    unsafe { *(self.header.as_ptr().add(MAGIC_9_DISPLAY_IDX) as *const [f32; 9]) };
                if m.iter().all(|&v| v == 0.) {
                    [1., 0., 0., 0., 1., 0., 0., 0., 1.]
                } else {
                    m
                }
            };
            let mut magic9inv: [u8; 8 * 9] = unsafe {
                let p = self.header.as_ptr().add(MAGIC_9_INV_IDX) as *const u8;
                let mut a = [0u8; 8 * 9];
                std::ptr::copy_nonoverlapping(p, a.as_mut_ptr(), 8 * 9);
                a
            };
            // magic9inv is the DNG ColorMatrix1 (9 SRATIONALs, num/den i32 pairs). It is only written by a calibration scan; pre-calibration shared memory is all zeros, which is a degenerate (0/0) matrix that raw converters reject. Fall back to the identity SRATIONAL pattern so uncalibrated DNGs remain valid.
            if magic9inv.iter().all(|&b| b == 0) {
                magic9inv = [
                    1, 0, 0, 0, 1, 0, 0, 0, //
                    0, 0, 0, 0, 1, 0, 0, 0, //
                    0, 0, 0, 0, 1, 0, 0, 0, //
                    0, 0, 0, 0, 1, 0, 0, 0, //
                    1, 0, 0, 0, 1, 0, 0, 0, //
                    0, 0, 0, 0, 1, 0, 0, 0, //
                    0, 0, 0, 0, 1, 0, 0, 0, //
                    0, 0, 0, 0, 1, 0, 0, 0, //
                    1, 0, 0, 0, 1, 0, 0, 0, //
                ];
            }

            // Calculate black level based on mode
            let image_black_level = match current_mode {
                // Average and Slitscan store raw counts rescaled to 16-bit (value<<16)/white with no black
                // subtraction, so the DNG carries the rescaled black; Diff/Motion are zero-centred magnitudes.
                0 | 3 => (sensor_black_level as u32 * 65536 / sensor_white_level as u32) as u16,
                _ => 0,
            };

            // Convert Android bayer pattern to CFA pattern
            // Android: 0=RGGB, 1=GRBG, 2=GBRG, 3=BGGR
            // CFA values: 0=Red, 1=Green, 2=Blue
            let base_cfa = match bayer_pattern {
                0 => [0u8, 1, 1, 2], // RGGB
                1 => [1, 0, 2, 1],   // GRBG
                2 => [1, 2, 0, 1],   // GBRG
                3 => [2, 1, 1, 0],   // BGGR
                _ => [0, 1, 1, 2],   // Default to RGGB
            };
            // RAW10 max-res on this device is a quad-Bayer (Tetracell) sensor: each base colour covers a 2x2 cluster, so the CFA is a 4x4 tile (each base cell expanded to 2x2). Tag the DNG with the real 4x4 pattern so a quad-Bayer-aware raw converter can demosaic it. Binned mode stays the standard 2x2 pattern.
            let bayer_pattern_vec: Vec<u8> = if raw10 {
                // base is [c00,c01,c10,c11] over a 2x2; expand to 4x4 row-major.
                let (c00, c01, c10, c11) = (base_cfa[0], base_cfa[1], base_cfa[2], base_cfa[3]);
                vec![
                    c00, c00, c01, c01, //
                    c00, c00, c01, c01, //
                    c10, c10, c11, c11, //
                    c10, c10, c11, c11, //
                ]
            } else {
                base_cfa.to_vec()
            };

            // Calculate data location based on mode
            let (data_offset, data_length) = match current_mode {
                0 => {
                    // Average: just average data
                    let avg_offset = (current_slot * 2) * pixel_count;
                    (avg_offset, pixel_count)
                }
                1 => {
                    // Difference: just difference data
                    let diff_offset = (current_slot * 2 + 1) * pixel_count;
                    (diff_offset, pixel_count)
                }
                2 => {
                    // Motion: both average and difference data
                    let avg_offset = (current_slot * 2) * pixel_count;
                    (avg_offset, pixel_count * 2)
                }
                3 => (0, 0), // Slitscan: raw_slot is assembled from the ring below, not a contiguous slot.
                _ => {
                    panic!("Unknown image mode");
                }
            };

            // Generate filename with image creation timestamp
            let mode_str = match current_mode {
                0 => "average",
                1 => "difference",
                2 => "motion",
                3 => "slitscan",
                _ => panic!("Unknown integration mode: {}", current_mode),
            };

            // Convert timestamp to human-readable format
            let datetime = DateTime::<Local>::from(self.last_image_timestamp);
            let timestamp_ms = self
                .last_image_timestamp
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let millis = timestamp_ms % 1000;

            // Base filename without extension; each encoder appends its own. Format: "YYYYMMDD HHMMSS mmm Mode" (compact date, compact time, milliseconds, then the integration mode last). No colons - they are ILLEGAL in Android/FUSE filenames (a DISPLAY_NAME with ':' yields a MediaStore row whose backing file can't be materialised, so it "saves" then vanishes when the scanner reaps the dangling row). Uses the CAPTURE timestamp (last_image_timestamp, set when the exposure started), so re-saving the same displayed frame produces the same filename and the existing-file check dedups it.
            let filename_base = format!(
                "{} {:03} {}",
                datetime.format("%Y%m%d %H%M%S"),
                millis,
                mode_str
            );

            // Copy the slot's raw u16 data for the save thread (the camera thread keeps overwriting the live slots). DNG needs the full data_length (avg, or avg+diff for motion); RGB exports debayer the average half. `mut` so the calibration correction can rewrite the avg region in place before any format branch.
            // Slitscan instead assembles the ring in chronological order (oldest row first): source row
            // (head + d) % ring_rows for output row d. The head advances by a whole Bayer period each frame
            // and ring_rows is a multiple of that period, so reordering whole rows preserves the CFA phase
            // (output row d gets a row whose phase is d % period) - the assembled strip is a valid raw image.
            let mut raw_slot: Vec<u16> = if is_slitscan {
                let rr = save_height;
                let w = save_width;
                let head = (self.header[SLITSCAN_HEAD_IDX] as usize) % rr.max(1);
                let mut v = vec![0u16; w * rr];
                for d in 0..rr {
                    let src = (head + d) % rr;
                    v[d * w..d * w + w].copy_from_slice(&self.slitscan_buffer[src * w..src * w + w]);
                }
                v
            } else {
                self.image_buffer[data_offset..data_offset + data_length].to_vec()
            };

            let width = save_width;
            let height = save_height;
            let display_orientation = device_orientation; // 0/1/2/3 -> 0/90/180/270 below

            // Calibration apply inputs for the save thread: dup the cal fds (the thread owns its copies, freed when read), the light's effective ISO + integration time (so the dark-current/offset terms scale right), and the cal->light value scale (65535/sensor_white). cal_bias_fd<0 = no cal loaded -> skip, save uncorrected.
            let (cal_bias_fd, cal_dark_fd) = if self.cal_bias_fd >= 0 && self.cal_dark_fd >= 0 {
                (unsafe { libc::dup(self.cal_bias_fd) }, unsafe { libc::dup(self.cal_dark_fd) })
            } else {
                (-1, -1)
            };
            let cal_iso_light = eff_iso;
            let cal_t_light_ns = integ_s * 1.0e9;
            let cal_scale = 65535.0 / (self.cal_white_level.max(1) as f64);

            thread::spawn(move || {
                use crate::image::save_encode::*;
                use crate::shared_memory::*;

                // device rotation degrees passed straight through (the RGB encoders rotate the pixels by
                // this, baking upright orientation into JPEG/TIFF/JXL).
                let orient_deg = match display_orientation {
                    90 => 90u16,
                    180 => 180,
                    270 => 270,
                    _ => 0,
                };

                // CALIBRATION: if a cal is loaded, decode the bias+dark VSFs (streamed from the dup'd fds, freed at end of scope - NOT held resident, which OOM-killed the process) and correct the avg region of raw_slot IN PLACE, before any format branch. So DNG (raw) and JPEG/TIFF/JXL (demosaiced from this same raw) are all calibrated from one corrected buffer. Motion mode (current_mode==2) uses the diff half and isn't a straight light frame, so skip it.
                if cal_bias_fd >= 0 && cal_dark_fd >= 0 && current_mode != 2 && current_mode != 3 {
                    use crate::image::calibration::{CalFile, LoadedCalibration};
                    let pc = width * height;
                    let bias = read_fd_bytes(cal_bias_fd);
                    let dark = read_fd_bytes(cal_dark_fd);
                    let loaded = bias.and_then(|b| CalFile::decode(&b, false)).and_then(|bf| {
                        dark.and_then(|d| CalFile::decode(&d, true)).and_then(|df| LoadedCalibration::from_pair(bf, df))
                    });
                    match loaded {
                        Some(cal) if cal.width == width && cal.height == height && raw_slot.len() >= pc => {
                            let corrected = cal.apply(&raw_slot[0..pc], cal_iso_light, cal_t_light_ns, cal_scale);
                            raw_slot[0..pc].copy_from_slice(&corrected);
                            log::info!("Calibration applied to {}x{} raw before encode (iso_cal={:.0} exp={:.0})", cal.width, cal.height, cal.iso_cal, cal.exposure_ns);
                        }
                        Some(cal) => log::warn!("Cal {}x{} != light {}x{}; saving uncorrected", cal.width, cal.height, width, height),
                        None => log::warn!("Cal decode failed; saving uncorrected"),
                    }
                }
                // Close the thread's dup'd fds (whether or not we used them).
                if cal_bias_fd >= 0 {
                    unsafe { libc::close(cal_bias_fd); }
                }
                if cal_dark_fd >= 0 {
                    unsafe { libc::close(cal_dark_fd); }
                }

                let (bytes, ext): (Vec<u8>, &str) = if save_format == SAVE_FORMAT_DNG {
                    // DNG: raw bayer + the real chameleon ColorMatrix1 (magic9inv) + D50.
                    // Build an embedded preview JPEG from the average half so any viewer/thumbnailer can show the photo without demosaicing the raw (esp. for quad-Bayer, which most tools can't read).
                    let avg_half = &raw_slot[0..(width * height).min(raw_slot.len())];
                    let (preview_jpeg, preview_dims) =
                        match crate::image::thumbnail::build_preview_jpeg(
                            avg_half,
                            width,
                            height,
                            image_black_level,
                            bayer_pattern,
                            raw10,
                            &display_matrix,
                            1024,
                            display_gain as f32,
                            orient_deg,
                        ) {
                            Some((bytes, pw, ph)) => (Some(bytes), (pw, ph)),
                            None => (None, (0, 0)),
                        };
                    let mut raw_info = RawInfo {
                        make: "Android".to_owned(),
                        makeoffset: 0,
                        makelen: 0,
                        model: "Lumis".to_owned(),
                        modeloffset: 0,
                        modellen: 0,
                        width,
                        height,
                        bitdepth: 16,
                        bitdepthold: 0,
                        rgb: false,
                        cfa: bayer_pattern_vec.clone(),
                        cfaw: if raw10 { 4 } else { 2 },
                        cfah: if raw10 { 4 } else { 2 },
                        black: if current_mode == 0 || current_mode == 3 {
                            image_black_level as f32
                        } else {
                            0.
                        },
                        blackoffset: 0,
                        blackcount: 0,
                        blacktype: 0,
                        white: 65535.,
                        // device rotation (degrees) -> EXIF Orientation tag. 0=1(normal), 90=6(rotate 90 CW),
                        // 180=3, 270=8(rotate 270 CW). Viewers rotate the native-orientation raw to match.
                        orientation: match display_orientation {
                            90 => 6,
                            180 => 3,
                            270 => 8,
                            _ => 1,
                        },
                        compression: false,
                        cam2terminal9: xyz_matrix,
                        magic9inv,
                        magicoffset: 0,
                        profileoffset: 0,
                        curveoffset: 0,
                        imagedataoffset: 0,
                        ifdoffset: 0,
                        duck: false,
                        save_scan: false,
                        cfapatternoffset: 0,
                        preview_jpeg,
                        preview_dims,
                        description: exposure_desc.clone(),
                        descriptionoffset: 0,
                        exif: exif.clone(),
                        exififdpointeroffset: 0,
                        gpsifdpointeroffset: 0,
                        baseline_exposure_stops,
                    };
                    let mut dng_bytes = make_base_dng(&mut raw_info);
                    let image_bytes = unsafe {
                        std::slice::from_raw_parts(
                            raw_slot.as_ptr() as *const u8,
                            raw_slot.len() * 2,
                        )
                    };
                    dng_bytes.extend_from_slice(image_bytes);

                    // Motion mode: compute motion in-place over the appended data, then trim.
                    if current_mode == 2 {
                        let pc = width * height;
                        let data_start = dng_bytes.len() - (pc * 4);
                        let motion = unsafe {
                            std::slice::from_raw_parts_mut(
                                dng_bytes[data_start..].as_mut_ptr() as *mut u16,
                                pc * 2,
                            )
                        };
                        for i in 0..pc {
                            let avg = motion[i] as u32;
                            let diff = motion[pc + i] as u32;
                            motion[i] = ((diff << 16)
                                / (avg.max(image_black_level as u32 + 1)
                                    - image_black_level as u32))
                                .min(65535) as u16;
                        }
                        dng_bytes.truncate(dng_bytes.len() - (pc * 2));
                    }
                    (dng_bytes, "dng")
                } else {
                    // RGB export: demosaic the average half + Rec.2020 display matrix + sqrt, then encode. Tagged Rec.2020 so it matches the on-screen preview. Quad-Bayer (max-res RAW10) uses the quad demosaic; standard Bayer uses RCD. TIFF takes a dedicated 16-bit path (error-diffusion-dithered) so the lossless export keeps the f32 pipeline's precision; JPEG/JPEG XL stay 8-bit.
                    let avg = &raw_slot[0..(width * height).min(raw_slot.len())];
                    let encoded = if save_format == SAVE_FORMAT_TIFF {
                        let (ow, oh, rgb16) = if raw10 {
                            quad_to_rgb16(
                                avg, width, height, image_black_level, bayer_pattern,
                                &display_matrix, display_gain as f32,
                            )
                        } else {
                            rcd_to_rgb16(
                                avg, width, height, image_black_level, bayer_pattern,
                                &display_matrix, display_gain as f32,
                            )
                        };
                        encode_tiff16(&rgb16, ow as u32, oh as u32, &exposure_desc, &exif).map(|b| (b, "tiff"))
                    } else {
                        let (ow, oh, rgb) = if raw10 {
                            quad_to_rgb8(
                                avg, width, height, image_black_level, bayer_pattern,
                                &display_matrix, display_gain as f32,
                            )
                        } else {
                            rcd_to_rgb8(
                                avg, width, height, image_black_level, bayer_pattern,
                                &display_matrix, display_gain as f32,
                            )
                        };
                        match save_format {
                            SAVE_FORMAT_JPEGXL => encode_jpegxl(&rgb, ow, oh, &exif).map(|b| (b, "jxl")),
                            _ => encode_jpeg(&rgb, ow as u32, oh as u32, &exposure_desc, &exif).map(|b| (b, "jpg")),
                        }
                    };
                    match encoded {
                        Some((b, e)) => (b, e),
                        None => {
                            log::error!("RGB encode failed for format {}", save_format);
                            return;
                        }
                    }
                };

                let filename = format!("{}.{}", filename_base, ext);
                set_pending_save_data(bytes, filename.clone());
                log::info!("Save data stored ({}), waiting for Kotlin", filename);
                // CURRENTLY_SAVING is cleared by Kotlin after the file is written.
            });
        }
    }

    /// One frame of a dark-frame calibration capture. Accumulates per-pixel mean (`accumulated`) and
    /// frame-to-frame variability (`difference`) without the normal per-exposure reset, maintains a
    /// sample-only even/odd half-sum for the live split-half convergence correlation, writes throttled
    /// progress stats to shared memory (gated at ~1.5s so 16s darks snapshot every frame while fast bias
    /// frames throttle), and finalizes to disk + stops when the UI sets CAL_FINALIZE_BIT.
    // Reset the calibration timing anchor to now. Called when CALIBRATING_BIT is engaged so the arrival
    // gate measures from the real cal start, and clear any prior cal state so the first frame re-inits.
    // Also stamps the unix start (EXPOSURE_START_*) the UI diffs for its LIVE elapsed counter - stamped
    // HERE (once, at engage) rather than on the first frame, so the timer doesn't reset ~16s in when the
    // first dark frame finally arrives and inits the cal state.
    pub fn mark_calibration_start(&mut self) {
        self.exposure_start_time = Instant::now();
        self.cal = None;
        if let Ok(t) = SystemTime::now().duration_since(UNIX_EPOCH) {
            self.header[EXPOSURE_START_SECS_IDX] = t.as_secs();
            self.header[EXPOSURE_START_NANOS_IDX] = t.subsec_nanos() as u64;
        }
    }

    /// One frame of a slitscan capture. A slitscan COLUMN is integrated exactly like a normal Average
    /// exposure - the centre Bayer-period band is summed into a u32 accumulator across frames until the set
    /// exposure time elapses - then averaged + scaled to 16-bit (the identical formula as process_frame's
    /// exposure-complete branch) and written into the ring, and the head advances one period. So every column
    /// is one full exposure's worth of light at the same frame_count / effective-ISO the other modes report,
    /// not a single free-running frame. The band is one full Bayer period tall so the assembled strip stays a
    /// valid demosaicable raw, and the ring is isolated from the slots so it never disturbs a held stack.
    fn process_slitscan_frame(&mut self, frame_data: &[u8], elapsed_ms: u64, force_completion: bool) {
        let w = self.width;
        let h = self.height;
        // Band height = one full Bayer period (4 rows for the quad 4x4 Tetracell, 2 for standard Bayer).
        let period = if self.header[QUAD_BAYER_IDX] != 0 { 4 } else { 2 };
        let strip_len = period * w;
        // Centre band, aligned DOWN to the Bayer period so the colour phase is identical every frame.
        let center = ((h / 2) / period) * period;
        // Time axis = the ring's own height: slitscan_buffer is exactly width x (2*width), so this is 2*w.
        let ring_rows = self.slitscan_buffer.len() / w.max(1);
        if ring_rows == 0 || frame_data.len() < (center + period) * w * 2 {
            return;
        }
        // Read the centre-band pixel at row r (0..period), column x out of frame_data.
        let band_px = |frame_data: &[u8], r: usize, x: usize| -> u32 {
            let off = (center + r) * w * 2 + x * 2;
            u16::from_le_bytes([frame_data[off], frame_data[off + 1]]) as u32
        };
        // First frame after (re-)entering slitscan: RESUME from wherever the ring left off. Do NOT clear the
        // strip or reset the head - switching to another mode and back must preserve the captured strip (the
        // whole point of the dedicated buffer; the ring just keeps rolling and overwrites the oldest columns in
        // time). Only seed the live column from this frame and restart the per-column exposure clock (the gap
        // spent in another mode must not instantly complete a column), and resume running.
        if !self.was_slitscan {
            self.header[FLAGS_IDX] &= !SLITSCAN_PAUSED_BIT; // re-entry resumes running
            if self.slitscan_accum.len() != strip_len {
                self.slitscan_accum = vec![0u32; strip_len];
            }
            for r in 0..period {
                for x in 0..w {
                    self.slitscan_accum[r * w + x] = band_px(frame_data, r, x);
                }
            }
            self.exposure_start_time = Instant::now();
            self.header[FRAME_COUNTER_IDX] = 0;
            self.was_slitscan = true;
            return;
        }
        // Bluetooth remote (force_completion) toggles run/pause. Pausing freezes the ring as-is so the strip
        // holds still to inspect/zoom/save; resuming re-seeds the live column from THIS frame (without clearing
        // the captured strip) and restarts the per-column clock, so the long paused gap doesn't instantly
        // complete a column.
        if force_completion {
            self.header[FLAGS_IDX] ^= SLITSCAN_PAUSED_BIT;
            if (self.header[FLAGS_IDX] & SLITSCAN_PAUSED_BIT) == 0 {
                for r in 0..period {
                    for x in 0..w {
                        self.slitscan_accum[r * w + x] = band_px(frame_data, r, x);
                    }
                }
                self.exposure_start_time = Instant::now();
                self.header[FRAME_COUNTER_IDX] = 0;
            }
            return;
        }
        // Paused: freeze - keep the ring and accumulator untouched.
        if (self.header[FLAGS_IDX] & SLITSCAN_PAUSED_BIT) != 0 {
            return;
        }
        if elapsed_ms >= self.exposure_time_ms {
            // Column complete: average (sum / frame_count) and scale to 16-bit EXACTLY like process_frame's
            // Average branch, write it at the head, advance, then seed the next column with this frame.
            let frame_count = self.header[FRAME_COUNTER_IDX].max(1);
            let white = self.white_level as u64;
            let head = (self.header[SLITSCAN_HEAD_IDX] as usize) % ring_rows;
            for r in 0..period {
                let dst_off = ((head + r) % ring_rows) * w;
                for x in 0..w {
                    let avg = ((self.slitscan_accum[r * w + x] as u64) << 16) / (white * frame_count);
                    self.slitscan_buffer[dst_off + x] = avg.min(65535) as u16;
                    self.slitscan_accum[r * w + x] = band_px(frame_data, r, x); // seed next column
                }
            }
            let new_head = (head + period) % ring_rows;
            self.header[SLITSCAN_HEAD_IDX] = new_head as u64;
            // A full ring rotation completes each time the head wraps back to 0; continuous-save keys off this.
            if new_head == 0 {
                self.slitscan_rotations += 1;
            }
            self.exposure_start_time = Instant::now();
            self.header[FRAME_COUNTER_IDX] = 0;
            // Bump the image counter so the UI does a full redraw and picks up the new column.
            self.header[IMAGE_COUNTER_IDX] += 1;
            self.last_image_timestamp = SystemTime::now();
        } else {
            // Still integrating this column: add the centre band to the accumulator.
            for r in 0..period {
                for x in 0..w {
                    self.slitscan_accum[r * w + x] += band_px(frame_data, r, x);
                }
            }
        }
    }

    fn process_calibration_frame(&mut self, frame_data: &[u8], captured_iso: i32, captured_shutter_ns: i64) {
        let pixel_count = self.width * self.height;

        // Arrival gate (wall-clock, HAL-metadata-independent). The HAL delivers an early frame before the
        // forced manual exposure takes effect - the tell is the frame counter jumping to 1 within ~0.5s of
        // starting a 16s dark. That fast first frame is NOT a real dark and would contaminate the average.
        // The per-frame SENSOR_EXPOSURE_TIME can't be trusted on that early frame (it often echoes the
        // requested value), so gate on inter-frame time instead, which the HAL can't fake:
        //   DARK: a real dark frame physically cannot arrive faster than its exposure. Require at least HALF
        //         the forced shutter to have elapsed since cal start (first frame) or the last accepted
        //         frame (16s -> reject anything under 8s; a 9s frame passes). Reject otherwise.
        //   BIAS: the forced shutter is the SHORTEST the sensor allows, so frames are legitimately fast and
        //         a timing gate can't distinguish the early frame. Instead just ignore the first second of
        //         spin-up, then sum everything (a short frame is exactly what a bias wants anyway).
        let forced_shutter_ns = f64::from_bits(self.header[SHUTTER_NS_IDX]);
        let is_dark = (self.header[FLAGS_IDX] & CAL_IS_DARK_BIT) != 0;
        let now = Instant::now();
        if is_dark {
            // Reference = last accepted frame if we have one, else cal start (exposure_start_time, stamped
            // when the integrator was constructed just before calibration began).
            let since = match self.cal.as_ref() {
                Some(c) => now.duration_since(c.last_accept).as_secs_f64(),
                None => now.duration_since(self.exposure_start_time).as_secs_f64(),
            };
            let min_gap_s = (forced_shutter_ns / 1.0e9) * 0.5;
            if since < min_gap_s {
                log::info!(
                    "Calibration(dark): rejecting frame {:.2}s after the last (need >= {:.2}s)",
                    since,
                    min_gap_s
                );
                return;
            }
        } else {
            // BIAS spin-up: drop everything in the first second after cal start.
            let since_start = now.duration_since(self.exposure_start_time).as_secs_f64();
            if since_start < 1.0 {
                log::info!(
                    "Calibration(bias): skipping spin-up frame at {:.2}s (< 1.0s)",
                    since_start
                );
                return;
            }
        }

        // First (matching) calibration frame: (re)initialise the accumulators and the correlation sample.
        if self.cal.is_none() {
            for i in 0..pixel_count {
                self.integration_buffer.accumulated[i] = 0;
                self.integration_buffer.difference[i] = 0;
                self.integration_buffer.prev_frame[i] = 0;
            }
            // Deterministic, evenly-spread sample of ~300k pixels (no RNG available in this env; a prime
            // stride over the frame gives a well-distributed fixed subset that's stable across frames).
            let target = 300_000usize.min(pixel_count);
            let stride = (pixel_count / target).max(1);
            let sample_idx: Vec<usize> = (0..pixel_count).step_by(stride).collect();
            let n = sample_idx.len();
            self.cal = Some(CalibrationState {
                frame_count: 0,
                start_time: Instant::now(),
                last_snapshot: Instant::now() - std::time::Duration::from_secs(10), // force a snapshot on frame 1
                last_accept: Instant::now(),
                sample_idx,
                even_sum: vec![0u64; n],
                odd_sum: vec![0u64; n],
                captured_iso: 0.0,
                captured_shutter_ns: 0.0,
            });
            // NOTE: the unix start (EXPOSURE_START_*) that the UI diffs for its live elapsed counter is
            // stamped in mark_calibration_start() at engage time, NOT here - stamping it on the first frame
            // reset the timer ~16s in (when the first dark frame finally arrived and ran this init).
        }

        // Accumulate this frame. Borrow split: read cal sample/index, write integration buffers.
        let chunks = frame_data.chunks_exact(2);
        let is_even = self.cal.as_ref().unwrap().frame_count % 2 == 0;
        for (i, chunk) in chunks.enumerate() {
            let pixel = u16::from_le_bytes([chunk[0], chunk[1]]);
            self.integration_buffer.accumulated[i] += pixel as u32;
            let diff = (pixel as i32 - self.integration_buffer.prev_frame[i] as i32).abs() as u32;
            self.integration_buffer.difference[i] += diff;
            self.integration_buffer.prev_frame[i] = pixel;
        }
        // Update the sample-only even/odd half-sums for the convergence correlation.
        {
            let cal = self.cal.as_mut().unwrap();
            for (k, &idx) in cal.sample_idx.iter().enumerate() {
                let chunk_off = idx * 2;
                let pixel =
                    u16::from_le_bytes([frame_data[chunk_off], frame_data[chunk_off + 1]]) as u64;
                if is_even {
                    cal.even_sum[k] += pixel;
                } else {
                    cal.odd_sum[k] += pixel;
                }
            }
            cal.frame_count += 1;
            cal.last_accept = now; // wall-clock arrival of this accepted frame; gates the next dark frame
            // Record the HAL's actually-applied ISO/shutter (last accepted frame wins - stable post-settle).
            // Stored in the cal VSF so apply-time scales by the gain/exposure this cal was really shot at.
            if captured_iso > 0 {
                cal.captured_iso = captured_iso as f64;
            }
            if captured_shutter_ns > 0 {
                cal.captured_shutter_ns = captured_shutter_ns as f64;
            }
        }

        // Throttled stats snapshot (~1.5s gate). For 16s darks every frame passes; fast bias throttles.
        let do_snapshot = {
            let cal = self.cal.as_ref().unwrap();
            cal.last_snapshot.elapsed().as_millis() >= 1500
        };
        if do_snapshot {
            self.write_calibration_stats();
            self.cal.as_mut().unwrap().last_snapshot = Instant::now();
        }

        // Finalize on request: average + variance written to disk, then stop.
        if (self.header[FLAGS_IDX] & CAL_FINALIZE_BIT) != 0 {
            self.finalize_calibration();
            self.header[FLAGS_IDX] &= !(CAL_FINALIZE_BIT | CALIBRATING_BIT);
            self.cal = None;
        }
    }

    /// Compute and publish the live calibration progress stats from the running accumulators.
    fn write_calibration_stats(&mut self) {
        let cal = self.cal.as_ref().unwrap();
        let n = cal.frame_count.max(1);
        // Even/odd half-sums -> per-sample averages, correlated to give split-half convergence.
        // even frames count = ceil(n/2), odd = floor(n/2).
        let even_n = ((cal.frame_count + 1) / 2).max(1) as f64;
        let odd_n = (cal.frame_count / 2).max(1) as f64;
        let m = cal.sample_idx.len();
        let (mut a, mut b) = (Vec::with_capacity(m), Vec::with_capacity(m));
        for k in 0..m {
            a.push(cal.even_sum[k] as f64 / even_n);
            b.push(cal.odd_sum[k] as f64 / odd_n);
        }
        let correlation = if cal.frame_count >= 2 { pearson(&a, &b) } else { 0.0 };

        // Mean dark level and residual noise std from the full-res accumulators (sampled for speed).
        let mut mean_sum = 0.0f64;
        let mut var_sum = 0.0f64;
        for &idx in &cal.sample_idx {
            let mean = self.integration_buffer.accumulated[idx] as f64 / n as f64;
            mean_sum += mean;
            // `difference` accumulates |frame - prev|; per-frame mean abs difference approximates noise.
            let noise = self.integration_buffer.difference[idx] as f64 / n as f64;
            var_sum += noise * noise;
        }
        let mean = mean_sum / m as f64;
        let noise = (var_sum / m as f64).sqrt();

        self.header[CAL_FRAME_COUNT_IDX] = cal.frame_count;
        self.header[CAL_ELAPSED_MS_IDX] = cal.start_time.elapsed().as_millis() as u64;
        self.header[CAL_CORRELATION_IDX] = correlation.to_bits();
        self.header[CAL_MEAN_IDX] = mean.to_bits();
        self.header[CAL_NOISE_IDX] = noise.to_bits();
    }

    /// Write the finalized calibration (per-pixel mean + variance maps) to disk. For now writes raw
    /// little-endian u16 .bin files to a fixed app-internal path so they can be pulled with ADB; the
    /// VSF encoding and the Kotlin-provided files-dir path are wired in a later step.
    fn finalize_calibration(&mut self) {
        let cal = self.cal.as_ref().unwrap();
        let n = cal.frame_count.max(1) as u32;
        let pixel_count = self.width * self.height;
        let is_dark = (self.header[FLAGS_IDX] & CAL_IS_DARK_BIT) != 0;
        let kind = if is_dark { "dark" } else { "bias" };

        // Per-pixel mean (average) and variance (mean abs frame-to-frame diff), u16.
        let mut mean_samples = vec![0u16; pixel_count];
        let mut var_samples = vec![0u16; pixel_count];
        for i in 0..pixel_count {
            mean_samples[i] = (self.integration_buffer.accumulated[i] / n).min(65535) as u16;
            var_samples[i] = (self.integration_buffer.difference[i] / n).min(65535) as u16;
        }

        // Store as a self-verifying VSF: both maps as 16-bit BitPackedTensors plus labeled metadata
        // (kind, dimensions, frame count). VSF's built-in checksum means a corrupt cal map is detected on
        // read rather than silently poisoning every corrected shot. Routed through the SAME pending-save
        // -> MediaStore pipe the DNGs use (a direct std::fs::write to /sdcard fails under scoped storage).
        // Deflate the raw u16 LE bytes of each map. A 50MP 16-bit dark frame is ~200MB uncompressed -
        // far too big to read back into a Java array (OOMs the verify), and wasteful since the data is
        // near-black (mostly tiny values), so it compresses ~20-50x. Stored as VsfType::v(b'z', ...); the
        // VSF provenance checksum still covers the compressed bytes. The harness inflates on read.
        let deflate = |samples: &[u16]| -> Vec<u8> {
            use flate2::write::ZlibEncoder;
            use flate2::Compression;
            use std::io::Write;
            let mut raw = Vec::with_capacity(samples.len() * 2);
            for &s in samples {
                raw.extend_from_slice(&s.to_le_bytes());
            }
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
            let _ = enc.write_all(&raw);
            enc.finish().unwrap_or_default()
        };
        let mean_z = deflate(&mean_samples);
        let var_z = deflate(&var_samples);
        // ISO + shutter this cal was actually shot at (HAL-reported; fall back to the forced max/extreme if
        // the HAL never populated them). Stored so apply-time scales by the real gain/exposure and never has
        // to re-query the sensor's max ISO (firmware updates can move the ISO range -> wrong gain ratio).
        let iso_cal = if cal.captured_iso > 0.0 {
            cal.captured_iso
        } else {
            f64::from_bits(self.header[MAX_ISO_IDX])
        };
        let exposure_ns = if cal.captured_shutter_ns > 0.0 {
            cal.captured_shutter_ns
        } else if is_dark {
            f64::from_bits(self.header[LONGEST_SHUTTER_NS_IDX])
        } else {
            f64::from_bits(self.header[SHORTEST_SHUTTER_NS_IDX])
        };
        log::info!("Calibration {kind}: iso_cal={:.0} exposure_ns={:.0}", iso_cal, exposure_ns);
        let vsf_bytes = vsf::vsf_builder::VsfBuilder::new()
            .add_section(
                "calibration",
                vec![
                    ("kind".to_string(), vsf::types::VsfType::a(kind.to_string())),
                    ("width".to_string(), vsf::types::VsfType::u6(self.width as u64)),
                    ("height".to_string(), vsf::types::VsfType::u6(self.height as u64)),
                    ("frame_count".to_string(), vsf::types::VsfType::u6(cal.frame_count)),
                    // iso/exposure as integers (round; ISO and ns are whole-number domains).
                    ("iso_cal".to_string(), vsf::types::VsfType::u6(iso_cal.round() as u64)),
                    ("exposure_ns".to_string(), vsf::types::VsfType::u6(exposure_ns.round() as u64)),
                    ("black_level".to_string(), vsf::types::VsfType::u6(self.black_level as u64)),
                    ("mean".to_string(), vsf::types::VsfType::v(b'z', mean_z)),
                    ("variance".to_string(), vsf::types::VsfType::v(b'z', var_z)),
                ],
            )
            .build();
        match vsf_bytes {
            Ok(bytes) => {
                // Include focal length in the name so different LENSES at the same resolution don't
                // collide (a cal is sensor-specific; same-res main vs ultrawide would otherwise overwrite
                // each other). Same lens + kind + res deliberately reuses the name (overwrites the old cal).
                let focal_mm = f64::from_bits(self.header[FOCAL_LENGTH_MM_IDX]);
                let filename = format!(
                    "cal_{kind}_{:.1}mm_{}x{}.vsf",
                    focal_mm, self.width, self.height
                );
                log::info!("Calibration VSF built ({} bytes) -> {}", bytes.len(), filename);
                set_pending_save_data(bytes, filename);
            }
            Err(e) => log::error!("VSF build failed: {}", e),
        }

        // Publish the averaged dark frame into the display image_buffer so the UI can show it (gamma 4)
        // after finalize. Write the per-pixel mean (u16) into slot 0's avg region and point the image
        // counter there, then set CAL_SHOW_RESULT_BIT so the UI renders the result-view until tapped.
        let frame = self.image_buffer.len() / 8; // 4 slots * 2 (avg+diff) = 8 regions
        if frame >= pixel_count {
            for i in 0..pixel_count {
                self.image_buffer[i] = (self.integration_buffer.accumulated[i] / n).min(65535) as u16;
            }
            self.header[IMAGE_COUNTER_IDX] = 0; // slot 0
            self.header[FLAGS_IDX] |= CAL_SHOW_RESULT_BIT;
        }

        if crate::DEBUG {
            log::info!("Calibration finalized: {} frames, kind={}", cal.frame_count, kind);
        }
    }

    /// Poll-driven finalize: if the UI has requested finalize (CAL_FINALIZE_BIT) and a calibration is
    /// running, average + write to disk and stop NOW, returning true. Called from the camera process's
    /// 30Hz settings poll so finalize happens within ~33ms instead of waiting for the next 16s frame.
    /// Safe to call between frames: during a long exposure the camera thread is blocked in Java waiting
    /// for the frame, not inside process_frame, so the accumulation buffers are not being mutated.
    pub fn check_and_finalize_calibration(&mut self) -> bool {
        if (self.header[FLAGS_IDX] & CALIBRATING_BIT) == 0 {
            return false;
        }
        if (self.header[FLAGS_IDX] & CAL_FINALIZE_BIT) == 0 {
            return false;
        }
        if self.cal.is_none() {
            // Finalize requested but no frames accumulated yet; just clear the flags and stop.
            self.header[FLAGS_IDX] &= !(CAL_FINALIZE_BIT | CALIBRATING_BIT);
            return true;
        }
        self.finalize_calibration();
        self.header[FLAGS_IDX] &= !(CAL_FINALIZE_BIT | CALIBRATING_BIT);
        self.cal = None;
        true
    }

    /// Read the current manual settings (ISO, shutter ns, focus) straight from shared memory without processing a frame. Lets Kotlin poll and push setting changes to the HAL at a fixed fast rate instead of only once per delivered frame - critical for long exposures, where frames arrive seconds apart and a frame-coupled update made dial changes take several frames to reach the capture request.
    pub fn current_settings(&self) -> (i32, i64, f32) {
        let iso = f64::from_bits(self.header[ISO_IDX]);
        let shutter_ns = f64::from_bits(self.header[SHUTTER_NS_IDX]);
        let focus = f64::from_bits(self.header[FOCUS_IDX]);
        (iso as i32, shutter_ns as i64, focus as f32)
    }

    pub fn get_shared_memory_ptr(&self) -> *mut u8 {
        self.header.as_ptr() as *mut u8
    }

    /// Get the shared memory file descriptor for IPC (Android only)
    pub fn get_shared_memory_fd(&self) -> Option<i32> {
        self.shared_memory.get_fd()
    }
}
