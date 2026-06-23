use crate::image::dng::*;
use crate::shared_memory::*;
use chrono::{DateTime, Local};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::{thread, u32};

// Global storage for pending save data (now just DNG bytes and filename)
static PENDING_SAVE_DATA: Mutex<Option<(Vec<u8>, String)>> = Mutex::new(None);

/// Get pending save data for JNI save operations
pub fn get_pending_save_data() -> Option<(Vec<u8>, String)> {
    PENDING_SAVE_DATA.lock().unwrap().take()
}

/// Set pending save data for JNI save operations  
pub fn set_pending_save_data(dng_data: Vec<u8>, filename: String) {
    *PENDING_SAVE_DATA.lock().unwrap() = Some((dng_data, filename));
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

pub struct CameraIntegrator {
    shared_memory: SharedMemory, // Keep SharedMemory alive
    pub header: &'static mut [u64],
    image_buffer: &'static mut [u16],
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

        // Seed ISO and per-frame shutter from the one-shot auto-exposure result that
        // Kotlin metered before creating this integrator, clamped to the sensor's range.
        // This makes the opening preview usable on any device/lighting without hardcoding
        // a value (sensor ranges vary wildly: min ISO 16 here could be 1 elsewhere, and
        // max exposure ≈16s here could be a full day). A bad seed previously defaulted
        // shutter to max_exposure_ns, so the feed delivered one frame every ~16s and
        // looked frozen. Fall back to min when the AE value is missing (<= 0).
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
        }
    }

    pub fn process_frame(
        &mut self,
        frame_data: &[u8],
        _captured_iso: i32,
        _captured_shutter_ns: i64,
        _captured_focus: f32,
    ) -> (i32, i64, f32) {
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

        // Check for save requests and spawn saving thread if needed
        let flags = self.header[FLAGS_IDX];
        let manual_save = (flags & MANUAL_SAVE_BIT) != 0;
        let continuous_save = (flags & CONTINUOUS_SAVE_BIT) != 0;
        let already_saving = (flags & CURRENTLY_SAVING) != 0;

        if !already_saving
            && ((continuous_save && (self.last_saved_counter != self.header[IMAGE_COUNTER_IDX]))
                || manual_save)
        {
            // Set the CURRENTLY_SAVING flag immediately to prevent duplicate saves
            self.header[FLAGS_IDX] |= CURRENTLY_SAVING;
            self.last_saved_counter = self.header[IMAGE_COUNTER_IDX];
            log::info!("Save requested");
            let current_slot = (self.header[IMAGE_COUNTER_IDX] & 3) as usize;
            let current_mode = self.header[CURRENT_MODE_IDX] as u8;
            let sensor_black_level = self.header[BLACK_LEVEL_IDX] as u16;
            let sensor_white_level = self.header[WHITE_LEVEL_IDX] as u16;
            let bayer_pattern = self.header[SENSOR_BAYER_PATTERN_IDX] as u32;
            let device_orientation = self.header[SENSOR_ORIENTATION_IDX] as u16;
            let pixel_count = self.width * self.height;
            let save_format = self.header[SAVE_FORMAT_IDX];
            // XYZ matrix for RGB exports, and the magic9inv bytes for the DNG ColorMatrix1.
            // magic_9_dng_xyz lives in zero-initialized shared memory and is only populated by
            // a calibration scan. Pre-calibration it is all zeros, which would multiply every
            // exported pixel to black; fall back to identity so uncalibrated RGB exports show
            // the raw debayered scene (accuracy doesn't matter until calibrated anyway).
            let xyz_matrix = {
                let m = *self.magic_9_dng_xyz;
                if m.iter().all(|&v| v == 0.) {
                    [1., 0., 0., 0., 1., 0., 0., 0., 1.]
                } else {
                    m
                }
            };
            // RGB exports (JPEG/TIFF/JXL) are tagged Rec.2020 to match the on-screen preview,
            // so they use the camera->Rec.2020 display matrix, NOT the DNG's XYZ matrix. Same
            // zero-fallback to identity for the uncalibrated case.
            let display_matrix = {
                let m = unsafe {
                    *(self.header.as_ptr().add(MAGIC_9_DISPLAY_IDX) as *const [f32; 9])
                };
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
            // magic9inv is the DNG ColorMatrix1 (9 SRATIONALs, num/den i32 pairs). It is only
            // written by a calibration scan; pre-calibration shared memory is all zeros, which
            // is a degenerate (0/0) matrix that raw converters reject. Fall back to the identity
            // SRATIONAL pattern so uncalibrated DNGs remain valid.
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
                0 => (sensor_black_level as u32 * 65536 / sensor_white_level as u32) as u16, // Average
                _ => 0, // Diff/Motion
            };

            // Convert Android bayer pattern to CFA pattern
            // Android: 0=RGGB, 1=GRBG, 2=GBRG, 3=BGGR
            // CFA values: 0=Red, 1=Green, 2=Blue
            let bayer_pattern_vec = match bayer_pattern {
                0 => vec![0, 1, 1, 2], // RGGB
                1 => vec![1, 0, 2, 1], // GRBG
                2 => vec![1, 2, 0, 1], // GBRG
                3 => vec![2, 1, 1, 0], // BGGR
                _ => vec![0, 1, 1, 2], // Default to RGGB
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
                _ => {
                    panic!("Unknown image mode");
                }
            };

            // Generate filename with image creation timestamp
            let mode_str = match current_mode {
                0 => "average",
                1 => "difference",
                2 => "motion",
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

            // Base filename without extension; each encoder appends its own.
            let filename_base = format!(
                "{} {} {:03}",
                mode_str,
                datetime.format("%Y-%m-%d %H:%M:%S"),
                millis
            );

            // Copy the slot's raw u16 data for the save thread (the camera thread keeps
            // overwriting the live slots). DNG needs the full data_length (avg, or avg+diff
            // for motion); RGB exports debayer the average half.
            let raw_slot: Vec<u16> =
                self.image_buffer[data_offset..data_offset + data_length].to_vec();

            let width = self.width;
            let height = self.height;
            let display_orientation = device_orientation; // 0/1/2/3 -> 0/90/180/270 below

            thread::spawn(move || {
                use crate::image::save_encode::*;
                use crate::shared_memory::*;

                let orient_deg = match display_orientation {
                    1 => 90u16,
                    2 => 180,
                    3 => 270,
                    _ => 0,
                };

                let (bytes, ext): (Vec<u8>, &str) = if save_format == SAVE_FORMAT_DNG {
                    // DNG: raw bayer + the real chameleon ColorMatrix1 (magic9inv) + D50.
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
                        cfaw: 2,
                        cfah: 2,
                        black: if current_mode == 0 {
                            image_black_level as f32
                        } else {
                            0.
                        },
                        blackoffset: 0,
                        blackcount: 0,
                        blacktype: 0,
                        white: 65535.,
                        orientation: match display_orientation {
                            0 => 1,
                            1 => 8,
                            2 => 3,
                            3 => 6,
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
                                / (avg.max(image_black_level as u32 + 1) - image_black_level as u32))
                                .min(65535) as u16;
                        }
                        dng_bytes.truncate(dng_bytes.len() - (pc * 2));
                    }
                    (dng_bytes, "dng")
                } else {
                    // RGB export: RCD-demosaic the average half + Rec.2020 display matrix +
                    // sqrt, then encode. Tagged Rec.2020 so it matches the on-screen preview.
                    let avg = &raw_slot[0..(width * height).min(raw_slot.len())];
                    let (ow, oh, rgb) = rcd_to_rgb8(
                        avg,
                        width,
                        height,
                        image_black_level,
                        bayer_pattern,
                        &display_matrix,
                        orient_deg,
                    );
                    let encoded = match save_format {
                        SAVE_FORMAT_TIFF => encode_tiff(&rgb, ow as u32, oh as u32).map(|b| (b, "tiff")),
                        SAVE_FORMAT_JPEGXL => encode_jpegxl(&rgb, ow, oh).map(|b| (b, "jxl")),
                        _ => encode_jpeg(&rgb, ow as u32, oh as u32).map(|b| (b, "jpg")),
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

    pub fn get_shared_memory_ptr(&self) -> *mut u8 {
        self.header.as_ptr() as *mut u8
    }

    /// Get the shared memory file descriptor for IPC (Android only)
    pub fn get_shared_memory_fd(&self) -> Option<i32> {
        self.shared_memory.get_fd()
    }
}
