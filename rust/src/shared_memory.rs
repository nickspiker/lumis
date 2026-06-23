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
pub const SAVE_FORMAT_IDX: usize = 37; // Save format: 0=JPEG, 1=TIFF, 2=DNG, 3=JPEG XL
pub const MAGIC_9_INV_IDX: usize = 38; // DNG magic9inv (72 bytes = 9 u64s, 38-46) from chameleon
pub const IMAGE_START: usize = 64;

// Save format values (SAVE_FORMAT_IDX)
pub const SAVE_FORMAT_JPEG: u64 = 0;
pub const SAVE_FORMAT_TIFF: u64 = 1;
pub const SAVE_FORMAT_DNG: u64 = 2;
pub const SAVE_FORMAT_JPEGXL: u64 = 3;
pub const SAVE_FORMAT_COUNT: u64 = 4;

// Boolean flag bit positions
pub const COMPLETE_EXPOSURE_BIT: u64 = 1 << 0;
pub const MANUAL_SAVE_BIT: u64 = 1 << 1;
pub const CONTINUOUS_SAVE_BIT: u64 = 1 << 2;
pub const CURRENTLY_SAVING: u64 = 1 << 3;

#[derive(Clone, Copy, PartialEq)]
pub enum RawMode {
    Average = 0,
    Difference = 1,
    Motion = 2,
}

impl From<u8> for RawMode {
    fn from(value: u8) -> Self {
        match value {
            0 => RawMode::Average,
            1 => RawMode::Difference,
            2 => RawMode::Motion,
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
