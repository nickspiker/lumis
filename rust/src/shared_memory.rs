pub const IMAGE_COUNTER_IDX: usize = 0; // RAW image counter, if incremented? new RAW image was written to shared memory!
pub const SENSOR_WIDTH_IDX: usize = 1;
pub const SENSOR_HEIGHT_IDX: usize = 2;
pub const SENSOR_BAYER_PATTERN_IDX: usize = 3;
pub const MIN_ISO_IDX: usize = 4;
pub const MAX_ISO_IDX: usize = 5;
pub const SHORTEST_SHUTTER_NS_IDX: usize = 6;
pub const LONGEST_SHUTTER_NS_IDX: usize = 7;
pub const MIN_FOCUS_IDX: usize = 8;
pub const WHITE_LEVEL_IDX: usize = 9;
pub const BLACK_LEVEL_IDX: usize = 10;
pub const CAMERA_FACING_IDX: usize = 11; // 0=back, 1=front
pub const SENSOR_ORIENTATION_IDX: usize = 12; // Sensor orientation in degrees (0, 90, 180, 270)
pub const SAVED_COUNTER_IDX: usize = 13;
pub const CURRENT_MODE_IDX: usize = 14; // 0=long exp, 1=diff sum, 2=motion
pub const FRAME_COUNTER_IDX: usize = 15; // number of frames thus far in the current exposure
pub const EXPOSURE_START_SECS_IDX: usize = 16;
pub const EXPOSURE_START_NANOS_IDX: usize = 17;
pub const FLAGS_IDX: usize = 18;
pub const ISO_IDX: usize = 19;
pub const SHUTTER_NS_IDX: usize = 20;
pub const FOCUS_IDX: usize = 21;
pub const EXPOSURE_TIME_MS_IDX: usize = 22;
pub const FPS_IDX: usize = 23;
pub const HEARTBEAT_SECS_IDX: usize = 24; // Seconds since Unix epoch for UI heartbeat
pub const HEARTBEAT_NANOS_IDX: usize = 25; // Nanoseconds within current second for UI heartbeat
pub const HISTOGRAM_COUNTER_IDX: usize = 26; // Histogram version counter for UI full draw triggering
pub const MAGIC_9_DISPLAY_IDX: usize = 27; // Display/Terminal magic 9 (9 f32s) + gamma (1 f32) = 5 u64s (27-31)
pub const MAGIC_9_DNG_XYZ_IDX: usize = 32; // Adobe XYZ magic 9 (9 f32s) + gamma (1 f32) = 5 u64s (32-36)
pub const SAVE_FORMAT_IDX: usize = 37; // Save format: 0=JPEG XL, 1=JPEG, 2=DNG, 3=TIFF
pub const MAGIC_9_INV_IDX: usize = 38; // DNG magic9inv (72 bytes = 9 u64s, 38-46) from chameleon
pub const QUAD_BAYER_IDX: usize = 47; // 1 if the frame is 4x4 quad-Bayer (max-res RAW10), else 0

// --- Dark-frame calibration capture (indices 48-55, free header space below IMAGE_START=64) ---
// The integrator, while CALIBRATING_BIT is set, accumulates frames without the normal per-exposure
// reset and writes these progress stats each frame so the calibration display screen can show how the
// capture is converging and let the user choose when to finalize.
pub const CAL_FRAME_COUNT_IDX: usize = 48; // u64: number of frames accumulated so far this calibration
pub const CAL_ELAPSED_MS_IDX: usize = 49; // u64: milliseconds since this calibration capture started
pub const CAL_CORRELATION_IDX: usize = 50; // f64 bits: split-half correlation (even vs odd frame averages); ~1.0 = clean fixed pattern
pub const CAL_MEAN_IDX: usize = 51; // f64 bits: mean dark level across the frame (raw counts)
pub const CAL_NOISE_IDX: usize = 52; // f64 bits: residual per-pixel noise std (falls ~1/sqrt(N) as it converges)

// JXL save support, probed by Kotlin at startup (some devices' MediaStore reject image/jxl). 0 = unknown
// (treat as supported - the default), 1 = supported, 2 = NOT supported (the save-format cycle skips JXL
// and the default falls back to JPEG). Numbered so the zero-init default doesn't accidentally disable JXL.
pub const JXL_SUPPORTED_IDX: usize = 53;

// Lens metadata for EXIF, set at camera init from CameraCharacteristics (f64 bits). 0 = unknown/omit.
pub const FOCAL_LENGTH_MM_IDX: usize = 54; // physical focal length in mm
pub const APERTURE_FNUM_IDX: usize = 55; // f-number (e.g. 1.8)
pub const SENSOR_DIAG_MM_IDX: usize = 56; // active-array physical diagonal in mm, for 35mm-equiv focal

// GPS for EXIF, set by Kotlin from last-known location (f64 bits). HAS_GPS=0 means no fix / permission
// denied -> omit GPS tags. Latitude/longitude in signed decimal degrees; altitude in metres.
pub const GPS_HAS_FIX_IDX: usize = 57; // 0 = no GPS, 1 = lat/lon/alt valid
pub const GPS_LAT_IDX: usize = 58; // signed decimal degrees
pub const GPS_LON_IDX: usize = 59; // signed decimal degrees
pub const GPS_ALT_IDX: usize = 60; // metres (signed; below sea level allowed)

// User display-gain multiplier (f64 bits), written by the UI so the save path can apply it: baked into
// RGB exports (JPEG/TIFF/JXL) and written as DNG BaselineExposure (log2(gain) stops, non-destructive).
// 0/unset is treated as 1.0 (no gain).
pub const DISPLAY_GAIN_IDX: usize = 61;

// Live device rotation in degrees (0/90/180/270) from the UI's gravity sensor - the same value that
// orients the on-screen controls. The save path reads it for the EXIF/DNG Orientation tag (and to rotate
// baked RGB exports), so saved files are oriented the way the phone was held. Sensor pixels stay native.
pub const DEVICE_ROTATION_IDX: usize = 62;

// Slitscan write-head: the ring row index (0..2*width) where the NEXT per-frame Bayer-period slice will be
// written. The camera process advances it each frame; the UI reads it to know "now" so it can scroll the
// strip and the save can assemble slices in chronological order (oldest = head, wrapping). Only meaningful
// in RawMode::Slitscan. Last free header slot before IMAGE_START.
pub const SLITSCAN_HEAD_IDX: usize = 63;

pub const IMAGE_START: usize = 64;

/// Slitscan ring length in u16 elements: a width x (2*width) strip (2:1 aspect). The ring lives in its OWN
/// region immediately after the 8 image_buffer planes, so the live slitscan capture never tramples a held
/// Average/Difference/Motion stack (those share the slots; slitscan does not touch them).
pub fn slitscan_ring_u16(width: usize) -> usize {
    2 * width * width
}

/// Slitscan ring size in bytes, rounded up to the segment's 8-byte alignment. The Kotlin UI side recomputes
/// the identical value (UserInterface.kt onCameraReady) so both processes map the same total segment size.
pub fn slitscan_ring_bytes(width: usize) -> usize {
    (slitscan_ring_u16(width) * 2 + 7) & !7
}

// Save format values (SAVE_FORMAT_IDX). Numbered to match the tap-cycle order JXL -> JPEG -> DNG -> TIFF, and JXL is 0 so it's the zero-initialized default.
pub const SAVE_FORMAT_JPEGXL: u64 = 0;
pub const SAVE_FORMAT_JPEG: u64 = 1;
pub const SAVE_FORMAT_DNG: u64 = 2;
pub const SAVE_FORMAT_TIFF: u64 = 3;
pub const SAVE_FORMAT_COUNT: u64 = 4;

// Boolean flag bit positions
pub const COMPLETE_EXPOSURE_BIT: u64 = 1 << 0;
pub const MANUAL_SAVE_BIT: u64 = 1 << 1;
pub const CONTINUOUS_SAVE_BIT: u64 = 1 << 2;
pub const CURRENTLY_SAVING: u64 = 1 << 3;
// Set while a dark-frame calibration capture is running: the integrator accumulates without the normal
// per-exposure reset (mean + per-pixel variance + even/odd half-sums for the convergence metric).
pub const CALIBRATING_BIT: u64 = 1 << 4;
// 1 = dark-current calibration (max ISO + longest shutter), 0 = bias (max ISO + shortest shutter).
pub const CAL_IS_DARK_BIT: u64 = 1 << 5;
// Set by the UI to request the running calibration be finalized (averaged + written to disk) and stopped.
pub const CAL_FINALIZE_BIT: u64 = 1 << 6;
// Set by the integrator after finalize: the averaged dark frame is in the display image_buffer slot and
// the UI should show it (bright, gamma ~4) until tapped. Tapping clears this and returns to the menu.
pub const CAL_SHOW_RESULT_BIT: u64 = 1 << 7;
// Read-back verification of the saved calibration VSF: Kotlin re-reads the written file and has Rust
// decode it (checksum + tensor integrity). OK or FAIL is shown on the result preview screen. Neither set
// = verify not done yet (e.g. still writing).
pub const CAL_VERIFY_OK_BIT: u64 = 1 << 8;
pub const CAL_VERIFY_FAIL_BIT: u64 = 1 << 9;
// Slitscan run/pause: the Bluetooth remote toggles this in slitscan mode. While set, the integrator freezes
// the ring (no integration, no head advance) so the captured strip holds still to inspect/zoom/save; toggling
// it off resumes capture from where it stopped. Cleared on entry to the mode so slitscan always starts running.
pub const SLITSCAN_PAUSED_BIT: u64 = 1 << 10;

#[derive(Clone, Copy, PartialEq)]
pub enum RawMode {
    Average = 0,
    Difference = 1,
    Motion = 2,
    Slitscan = 3,
}

impl From<u8> for RawMode {
    fn from(value: u8) -> Self {
        match value {
            0 => RawMode::Average,
            1 => RawMode::Difference,
            2 => RawMode::Motion,
            3 => RawMode::Slitscan,
            _ => RawMode::Average, // Default fallback
        }
    }
}

pub struct SharedMemory {
    ptr: *mut u64,
    len: usize,
    owned: bool,     // Track if we allocated the memory
    fd: Option<i32>, // File descriptor for Android shared memory
}

// Magic9 helper functions for safe conversion and access
impl SharedMemory {
    pub fn write_magic_9_display(&mut self, magic9: &[f32; 9], gamma: f32) {
        let slice = self.as_slice();
        unsafe {
            let magic9_ptr = &mut slice[MAGIC_9_DISPLAY_IDX] as *mut u64 as *mut f32;
            let magic9_slice = std::slice::from_raw_parts_mut(magic9_ptr, 10);

            magic9_slice[0..9].copy_from_slice(magic9);
            magic9_slice[9] = gamma;
        }
    }

    pub fn write_magic_9_dng_xyz(&mut self, magic9: &[f32; 9], gamma: f32) {
        let slice = self.as_slice();
        unsafe {
            let magic9_ptr = &mut slice[MAGIC_9_DNG_XYZ_IDX] as *mut u64 as *mut f32;
            let magic9_slice = std::slice::from_raw_parts_mut(magic9_ptr, 10);
            magic9_slice[0..9].copy_from_slice(magic9);
            magic9_slice[9] = gamma;
        }
    }

    pub fn read_magic_9_display(&self) -> ([f32; 9], f32) {
        unsafe {
            let magic9_ptr = self.ptr.add(MAGIC_9_DISPLAY_IDX) as *const f32;
            let magic9_slice = std::slice::from_raw_parts(magic9_ptr, 10);

            let mut magic9 = [0.0f32; 9];
            magic9.copy_from_slice(&magic9_slice[0..9]);
            let gamma = magic9_slice[9];

            (magic9, gamma)
        }
    }

    /// Read Adobe XYZ magic9 matrix and gamma from shared memory (read access - any thread)
    pub fn read_magic_9_dng_xyz(&self) -> ([f32; 9], f32) {
        unsafe {
            // Cast the 5 u64s starting at MAGIC9_ADOBE_XYZ_IDX to f32 array
            let magic9_ptr = self.ptr.add(MAGIC_9_DNG_XYZ_IDX) as *const f32;
            let magic9_slice = std::slice::from_raw_parts(magic9_ptr, 10);

            let mut magic9 = [0.0f32; 9];
            magic9.copy_from_slice(&magic9_slice[0..9]);
            let gamma = magic9_slice[9];

            (magic9, gamma)
        }
    }

    /// Create new SharedMemory with internally allocated memory
    pub fn create(size_bytes: usize) -> Self {
        if crate::DEBUG && size_bytes % 8 != 0 {
            panic!(
                "SharedMemory size must be 8-byte aligned, got {} bytes",
                size_bytes
            );
        }

        let u64_count = size_bytes / 8;

        // Allocate aligned memory
        let layout = std::alloc::Layout::from_size_align(size_bytes, 8)
            .expect("Failed to create memory layout");
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };

        if ptr.is_null() {
            panic!("Failed to allocate {} bytes for SharedMemory", size_bytes);
        }

        if crate::DEBUG {
            log::info!(
                "SharedMemory allocated: {} bytes = {} u64s at 0x{:x}",
                size_bytes,
                u64_count,
                ptr as usize
            );
        }

        Self {
            ptr: ptr as *mut u64,
            len: u64_count,
            owned: true,
            fd: None,
        }
    }

    /// Create SharedMemory using Android ASharedMemory
    #[cfg(target_os = "android")]
    pub fn create_ashmem(size_bytes: usize) -> Self {
        use libc::{mmap, MAP_SHARED, PROT_READ, PROT_WRITE};

        if crate::DEBUG && size_bytes % 8 != 0 {
            panic!(
                "SharedMemory size must be 8-byte aligned, got {} bytes",
                size_bytes
            );
        }

        let u64_count = size_bytes / 8;

        // Create ASharedMemory
        // Android API 26+ has ASharedMemory_create
        let fd = unsafe {
            // Using libc to call ASharedMemory_create
            let name = std::ffi::CString::new("lumis_shared_memory").unwrap();
            // Note: We'll need to link with libandroid for this
            extern "C" {
                fn ASharedMemory_create(
                    name: *const libc::c_char,
                    size: libc::size_t,
                ) -> libc::c_int;
            }
            ASharedMemory_create(name.as_ptr(), size_bytes)
        };

        if fd < 0 {
            panic!(
                "Failed to create ASharedMemory: {}",
                std::io::Error::last_os_error()
            );
        }

        // Map the shared memory
        let ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                size_bytes,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };

        if ptr == libc::MAP_FAILED {
            panic!(
                "Failed to map ASharedMemory: {}",
                std::io::Error::last_os_error()
            );
        }

        if crate::DEBUG {
            log::info!(
                "ASharedMemory created: {} bytes = {} u64s at 0x{:x}, fd={}",
                size_bytes,
                u64_count,
                ptr as usize,
                fd
            );
        }

        Self {
            ptr: ptr as *mut u64,
            len: u64_count,
            owned: true,
            fd: Some(fd),
        }
    }

    /// Map existing ASharedMemory from file descriptor
    #[cfg(target_os = "android")]
    pub fn from_ashmem_fd(fd: i32, size_bytes: usize) -> Self {
        use libc::{mmap, MAP_SHARED, PROT_READ, PROT_WRITE};

        if crate::DEBUG && size_bytes % 8 != 0 {
            panic!(
                "SharedMemory size must be 8-byte aligned, got {} bytes",
                size_bytes
            );
        }

        let u64_count = size_bytes / 8;

        // Map the shared memory
        let ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                size_bytes,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };

        if ptr == libc::MAP_FAILED {
            panic!(
                "Failed to map ASharedMemory from fd {}: {}",
                fd,
                std::io::Error::last_os_error()
            );
        }

        if crate::DEBUG {
            log::info!(
                "ASharedMemory mapped from fd: {} bytes = {} u64s at 0x{:x}, fd={}",
                size_bytes,
                u64_count,
                ptr as usize,
                fd
            );
        }

        Self {
            ptr: ptr as *mut u64,
            len: u64_count,
            owned: false, // Don't close fd when dropped - it's not ours
            fd: Some(fd),
        }
    }

    /// Get the file descriptor for IPC
    pub fn get_fd(&self) -> Option<i32> {
        self.fd
    }

    /// Get raw pointer for direct memory access (unsafe)
    pub fn get_ptr(&self) -> *mut u64 {
        self.ptr
    }

    /// Get mutable references to display/terminal magic 9 array and gamma with static lifetime
    pub fn get_magic_9_display_slices(&self) -> (&'static mut [f32; 9], &'static mut f32) {
        unsafe {
            let ptr = self.ptr.add(MAGIC_9_DISPLAY_IDX) as *mut f32;
            let array_ref = &mut *(ptr as *mut [f32; 9]);
            let gamma_ref = &mut *ptr.add(9);
            (
                std::mem::transmute::<&mut [f32; 9], &'static mut [f32; 9]>(array_ref),
                std::mem::transmute::<&mut f32, &'static mut f32>(gamma_ref),
            )
        }
    }

    /// Get mutable references to DNG XYZ magic 9 array and gamma with static lifetime
    pub fn get_magic_9_dng_xyz_slices(&self) -> (&'static mut [f32; 9], &'static mut f32) {
        unsafe {
            let ptr = self.ptr.add(MAGIC_9_DNG_XYZ_IDX) as *mut f32;
            let array_ref = &mut *(ptr as *mut [f32; 9]);
            let gamma_ref = &mut *ptr.add(9);
            (
                std::mem::transmute::<&mut [f32; 9], &'static mut [f32; 9]>(array_ref),
                std::mem::transmute::<&mut f32, &'static mut f32>(gamma_ref),
            )
        }
    }

    pub fn as_slice(&mut self) -> &mut [u64] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    /// Get mutable image data as u16 slice
    pub fn image_buffer(&mut self, pixel_count: usize) -> &mut [u16] {
        let mem = self.as_slice();
        unsafe {
            let image_ptr = &mut mem[IMAGE_START] as *mut u64 as *mut u16;
            std::slice::from_raw_parts_mut(image_ptr, pixel_count * 8)
        }
    }

    /// The slitscan ring: a width x (2*width) u16 region immediately AFTER the 8 image_buffer planes.
    /// Isolated from the slots, so slitscan capture and a held Average/Difference/Motion stack never collide.
    pub fn slitscan_buffer(&mut self, pixel_count: usize, width: usize) -> &mut [u16] {
        let mem = self.as_slice();
        unsafe {
            let image_ptr = &mut mem[IMAGE_START] as *mut u64 as *mut u16;
            let ring_ptr = image_ptr.add(pixel_count * 8);
            std::slice::from_raw_parts_mut(ring_ptr, slitscan_ring_u16(width))
        }
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        let size_bytes = self.len * 8;

        if let Some(fd) = self.fd {
            // Unmap ASharedMemory
            unsafe {
                libc::munmap(self.ptr as *mut libc::c_void, size_bytes);
            }

            // Close fd only if we own it
            if self.owned {
                unsafe {
                    libc::close(fd);
                }
            }

            if crate::DEBUG {
                log::info!(
                    "ASharedMemory unmapped: {} bytes at 0x{:x}, fd={} (owned={})",
                    size_bytes,
                    self.ptr as usize,
                    fd,
                    self.owned
                );
            }
        } else if self.owned {
            // Regular heap allocation
            let layout = std::alloc::Layout::from_size_align(size_bytes, 8)
                .expect("Failed to create memory layout for deallocation");
            unsafe {
                std::alloc::dealloc(self.ptr as *mut u8, layout);
            }
            if crate::DEBUG {
                log::info!(
                    "SharedMemory deallocated: {} bytes at 0x{:x}",
                    size_bytes,
                    self.ptr as usize
                );
            }
        }
    }
}
