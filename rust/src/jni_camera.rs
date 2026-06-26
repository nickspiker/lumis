use crate::image::integrator::*;
use crate::shared_memory::*;
extern crate libc;
use jni::{
    objects::{JByteBuffer, JClass, JObject},
    sys::{jboolean, jfloat, jint, jlong, jobject},
    JNIEnv,
};
use log::*;

fn integrator_ptr(ptr: jlong) -> &'static mut CameraIntegrator {
    if ptr == 0 {
        panic!("Null camera context pointer received");
    }
    unsafe { &mut *(ptr as *mut CameraIntegrator) }
}

/// Read-back verify a just-written calibration VSF. Kotlin passes the open file's descriptor (dup'd from
/// the MediaStore content URI) - we read the file in NATIVE memory here rather than slurping it into a
/// Java ByteArray, which at max-res is ~94MB and OOMs the camera process's 268MB Java heap (it already
/// holds the cal maps). We VSF-decode (validates the checksum) and confirm the mean/variance tensors
/// unpack to the expected pixel count, then set CAL_VERIFY_OK_BIT/CAL_VERIFY_FAIL_BIT for the result
/// screen. Returns true if verified OK. The fd is owned by Kotlin (it closes its stream); we only read.
#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeVerifyCalVsf<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    ptr: jlong,
    fd: jint,
) -> jboolean {
    let integrator = integrator_ptr(ptr);
    let bytes = match read_fd_to_vec(fd) {
        Ok(b) => b,
        Err(e) => {
            error!("Calibration verify: failed to read fd {}: {}", fd, e);
            integrator.header[FLAGS_IDX] |= CAL_VERIFY_FAIL_BIT;
            return 0;
        }
    };
    let ok = verify_cal_vsf(&bytes);
    integrator.header[FLAGS_IDX] &= !(CAL_VERIFY_OK_BIT | CAL_VERIFY_FAIL_BIT);
    if ok {
        integrator.header[FLAGS_IDX] |= CAL_VERIFY_OK_BIT;
        info!("Calibration VSF read-back verified OK");
        1
    } else {
        integrator.header[FLAGS_IDX] |= CAL_VERIFY_FAIL_BIT;
        error!("Calibration VSF read-back verification FAILED");
        0
    }
}

/// Read an entire file referenced by a raw fd into a Vec, in native memory. dup()s the fd so our File
/// owns its own descriptor and Kotlin remains free to close the stream it passed; reading starts from the
/// current offset (Kotlin rewinds / passes a fresh stream). Used by the cal verify to avoid a 94MB Java
/// ByteArray that OOMs the camera heap.
fn read_fd_to_vec(fd: jint) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    let dup = unsafe { libc::dup(fd) };
    if dup < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(dup) };
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?; // File drop closes `dup`, not the caller's fd
    Ok(buf)
}

/// Decode a calibration VSF and confirm the checksum + that the mean and variance tensors are present and
/// unpack to width*height samples (read from the section's own width/height fields).
fn verify_cal_vsf(bytes: &[u8]) -> bool {
    use vsf::file_format::{VsfHeader, VsfSection};
    use vsf::types::VsfType;
    let header = match VsfHeader::decode(bytes) {
        Ok((h, _)) => h,
        Err(_) => return false, // checksum / structure invalid
    };
    let field = match header.fields.iter().find(|f| f.name == "calibration") {
        Some(f) => f,
        None => return false,
    };
    let mut ptr = field.offset_bytes;
    let section = match VsfSection::parse(bytes, &mut ptr) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let get_u64 = |name: &str| -> Option<u64> {
        match section.get_field(name).and_then(|f| f.values.first()) {
            Some(VsfType::u6(v)) => Some(*v),
            _ => None,
        }
    };
    let (w, h) = match (get_u64("width"), get_u64("height")) {
        (Some(w), Some(h)) => (w as usize, h as usize),
        _ => return false,
    };
    let expected_bytes = w * h * 2; // u16 LE
    // Maps are deflate-compressed blobs (VsfType::v(b'z', ...)). Verify each inflates to width*height*2.
    let blob_ok = |name: &str| -> bool {
        match section.get_field(name).and_then(|f| f.values.first()) {
            Some(VsfType::v(b'z', compressed)) => {
                use flate2::read::ZlibDecoder;
                use std::io::Read;
                let mut dec = ZlibDecoder::new(&compressed[..]);
                let mut out = Vec::new();
                dec.read_to_end(&mut out).is_ok() && out.len() == expected_bytes
            }
            _ => false,
        }
    };
    blob_ok("mean") && blob_ok("variance")
}

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeCameraInit(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    width: jint,
    height: jint,
    white_level: jint,
    black_level: jint,
    bayer_pattern: jint,
    camera_facing: jint, // 0=back, 1=front
    sensor_orientation: jint,
    min_iso: jint,
    max_iso: jint,
    min_exposure: jlong,
    max_exposure: jlong,
    min_focus: jfloat,
    initial_iso: jint,
    initial_shutter_ns: jlong,
    focal_length_mm: jfloat,
    aperture_fnum: jfloat,
    sensor_diag_mm: jfloat,
) -> jlong {
    if crate::DEBUG {
        info!(
            "Initializing camera integrator: {}x{}, white={}, black={}, bayer={}, facing={}",
            width, height, white_level, black_level, bayer_pattern, camera_facing
        );
        info!(
            "Camera ranges: ISO {}-{}, exposure {}-{}ns",
            min_iso, max_iso, min_exposure, max_exposure
        );
    }

    // Calculate SharedMemory size needed
    let pixel_count = (width * height) as usize;
    let raw_buffer_size_bytes = pixel_count * 16; // 2 u16 arrays, 4 bytes per pixel, quad rolling buffer
    let header_size_bytes = crate::shared_memory::IMAGE_START * 8; // header u64s
    let total_size = header_size_bytes + raw_buffer_size_bytes;

    if crate::DEBUG {
        info!(
            "Creating camera integrator with SharedMemory size: {} bytes",
            total_size
        );
    }

    // Create CameraIntegrator - will need 2 frames to initialize buffers
    let integrator = Box::new(CameraIntegrator::new(
        width as usize,
        height as usize,
        white_level as u16,
        black_level as u16,
        bayer_pattern as u32,
        camera_facing as u32,
        sensor_orientation,
        total_size,
        min_iso as f64,
        max_iso as f64,
        min_exposure as f64,
        max_exposure as f64,
        min_focus as f64,
        initial_iso as f64,
        initial_shutter_ns as f64,
    ));

    // Lens metadata for EXIF (written straight to the header; only read at save time). 0 = unknown/omit.
    let mut integrator = integrator;
    integrator.header[crate::shared_memory::FOCAL_LENGTH_MM_IDX] = (focal_length_mm as f64).to_bits();
    integrator.header[crate::shared_memory::APERTURE_FNUM_IDX] = (aperture_fnum as f64).to_bits();
    integrator.header[crate::shared_memory::SENSOR_DIAG_MM_IDX] = (sensor_diag_mm as f64).to_bits();

    let integrator_ptr = Box::into_raw(integrator) as jlong;
    if crate::DEBUG {
        info!(
            "Camera integrator created at 0x{:x} (focal={}mm f/{} diag={}mm)",
            integrator_ptr as u64, focal_length_mm, aperture_fnum, sensor_diag_mm
        );
    }
    integrator_ptr
}

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeCameraGetSharedMemoryPtr(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    ptr: jlong,
) -> jlong {
    let integrator = integrator_ptr(ptr);

    let shared_memory_ptr = integrator.get_shared_memory_ptr();
    if crate::DEBUG {
        info!(
            "Returning SharedMemory pointer: 0x{:x}",
            shared_memory_ptr as u64
        );
    }
    shared_memory_ptr as jlong
}

/// Engage dark-frame calibration capture on this integrator: sets CALIBRATING_BIT (and CAL_IS_DARK_BIT
/// for a dark-current frame, clearing it for bias) so process_frame routes to the no-reset accumulate
/// path that publishes mean/variance + convergence stats. The forced ISO/shutter were already seeded at
/// init; the settings poll keeps pushing them to the HAL.
#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeSetCalibrationMode(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    ptr: jlong,
    dark: jboolean,
) {
    use crate::shared_memory::{CALIBRATING_BIT, CAL_IS_DARK_BIT, FLAGS_IDX};
    let integrator = integrator_ptr(ptr);
    integrator.header[FLAGS_IDX] |= CALIBRATING_BIT;
    if dark != 0 {
        integrator.header[FLAGS_IDX] |= CAL_IS_DARK_BIT;
    } else {
        integrator.header[FLAGS_IDX] &= !CAL_IS_DARK_BIT;
    }
    // Anchor the arrival gate to the moment calibration engages (not integrator construction, which can be
    // seconds earlier due to AE warm-up). The dark gate measures the first frame against this; the bias
    // spin-up window (first 1s ignored) is measured against it too.
    integrator.mark_calibration_start();
    info!(
        "Calibration mode engaged: {}",
        if dark != 0 { "DARK" } else { "BIAS" }
    );
}

/// Poll-driven calibration finalize check (camera process). Returns true if it finalized this call, so
/// Kotlin can transition the UI out of calibration. Called from the 30Hz settings poll so a FINALIZE
/// tap (which set CAL_FINALIZE_BIT via shared memory from the UI process) takes effect within ~33ms
/// instead of waiting for the next 16s frame.
#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeCheckFinalizeCalibration(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    ptr: jlong,
) -> jboolean {
    let integrator = integrator_ptr(ptr);
    if integrator.check_and_finalize_calibration() {
        1
    } else {
        0
    }
}

/// Record whether this device's MediaStore accepts image/jxl (probed once by Kotlin at startup). Writes
/// JXL_SUPPORTED_IDX = 1 (supported) or 2 (not), so the UI save-format cycle skips JXL and the save path
/// falls back to JPEG on devices that reject it.
#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeSetJxlSupported(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    ptr: jlong,
    supported: jboolean,
) {
    use crate::shared_memory::*;
    let integrator = integrator_ptr(ptr);
    integrator.header[JXL_SUPPORTED_IDX] = if supported != 0 { 1 } else { 2 };
    // JXL is the zero-init default (format 0). If it's unsupported, move the selected format off JXL to
    // JPEG so the on-screen indicator matches what actually gets saved (otherwise it shows "JXL" while
    // the save silently falls back to JPEG - confusing).
    if supported == 0 && integrator.header[SAVE_FORMAT_IDX] == SAVE_FORMAT_JPEGXL {
        integrator.header[SAVE_FORMAT_IDX] = SAVE_FORMAT_JPEG;
    }
}

/// Set the GPS fix (from Kotlin's last-known location) into the header for EXIF geotagging. has_fix=0
/// clears it (no permission / no location), so GPS tags are omitted.
#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeSetGps(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    ptr: jlong,
    has_fix: jboolean,
    lat: f64,
    lon: f64,
    alt: f64,
) {
    use crate::shared_memory::*;
    let integrator = integrator_ptr(ptr);
    integrator.header[GPS_HAS_FIX_IDX] = if has_fix != 0 { 1 } else { 0 };
    integrator.header[GPS_LAT_IDX] = lat.to_bits();
    integrator.header[GPS_LON_IDX] = lon.to_bits();
    integrator.header[GPS_ALT_IDX] = alt.to_bits();
}

/// Poll-driven save check (camera process). Runs try_save from the 30Hz settings poll so a manual save
/// at a long exposure fires within ~33ms (from the already-published image_buffer) instead of waiting up
/// to a full exposure for the next frame - the data already exists; only the flag check was frame-gated.
#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeCheckSave(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    ptr: jlong,
) {
    integrator_ptr(ptr).try_save();
}

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeCameraGetSharedMemoryFd(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    ptr: jlong,
) -> jint {
    let integrator = integrator_ptr(ptr);

    if let Some(fd) = integrator.get_shared_memory_fd() {
        if crate::DEBUG {
            info!("Returning SharedMemory file descriptor: {}", fd);
        }
        fd as jint
    } else {
        error!("SharedMemory was not created with ASharedMemory - no FD available");
        -1
    }
}

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeCameraGetWidth(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    ptr: jlong,
) -> jint {
    let integrator = integrator_ptr(ptr);

    integrator.width as jint
}

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeCameraGetHeight(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    ptr: jlong,
) -> jint {
    let integrator = integrator_ptr(ptr);

    integrator.height as jint
}

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeCameraOnFrame<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    ptr: jlong,
    buffer: JObject<'local>,
    captured_iso: jint,
    captured_shutter_ns: jlong,
    captured_focus_distance: jfloat,
    raw10: jboolean,
    row_stride: jint,
) -> JObject<'local> {
    let integrator = unsafe { &mut *(ptr as *mut CameraIntegrator) };

    let buffer = JByteBuffer::from(buffer);
    let Ok(data) = env.get_direct_buffer_address(&buffer) else {
        error!("PANIC: Failed to get direct buffer address in camera processing");
        panic!("Buffer access failed: Unable to get direct buffer address in camera processing");
    };
    let Ok(length) = env.get_direct_buffer_capacity(&buffer) else {
        error!("PANIC: Failed to get buffer capacity in camera processing");
        panic!("Buffer access failed: Unable to get buffer capacity in camera processing");
    };

    let frame_data = unsafe { std::slice::from_raw_parts(data, length as usize) };

    let (iso, shutter_ns, focus) = integrator.process_frame(
        frame_data,
        captured_iso,
        captured_shutter_ns,
        captured_focus_distance,
        raw10 != 0,
        row_stride as usize,
    );

    // Create CameraSettings object
    let settings_class = env
        .find_class("com/lumis/camera/CameraSettings")
        .expect("Failed to find CameraSettings class");

    let settings_obj = env
        .new_object(
            settings_class,
            "(IJF)V", // Constructor signature: (int, long, float)
            &[
                jni::objects::JValue::Int(iso),
                jni::objects::JValue::Long(shutter_ns),
                jni::objects::JValue::Float(focus),
            ],
        )
        .expect("Failed to create CameraSettings object");

    settings_obj
}

/// Return the current manual settings (ISO, shutter ns, focus) from shared memory WITHOUT processing a frame. Kotlin polls this at a fixed fast rate so a dial change reaches the capture request immediately, instead of waiting for the next delivered frame (which at long exposures can be seconds away - the cause of settings taking several frames to switch).
#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeGetCurrentSettings<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    ptr: jlong,
) -> JObject<'local> {
    let integrator = unsafe { &mut *(ptr as *mut CameraIntegrator) };
    let (iso, shutter_ns, focus) = integrator.current_settings();

    let settings_class = env
        .find_class("com/lumis/camera/CameraSettings")
        .expect("Failed to find CameraSettings class");
    env.new_object(
        settings_class,
        "(IJF)V",
        &[
            jni::objects::JValue::Int(iso),
            jni::objects::JValue::Long(shutter_ns),
            jni::objects::JValue::Float(focus),
        ],
    )
    .expect("Failed to create CameraSettings object")
}

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeGetSavedDngData<'local>(
    mut env: JNIEnv<'local>,
    _this: JObject<'local>,
) -> jobject {
    // Check if there's pending DNG save data
    match crate::image::integrator::get_pending_save_data() {
        Some((dng_data, filename)) => {
            if crate::DEBUG {
                info!(
                    "Returning DNG data to Kotlin: {} bytes, filename: {}",
                    dng_data.len(),
                    filename
                );
            }

            // Create byte array
            let byte_array = match env.new_byte_array(dng_data.len() as i32) {
                Ok(array) => array,
                Err(e) => {
                    error!("Failed to create byte array: {:?}", e);
                    return std::ptr::null_mut();
                }
            };

            if let Err(e) = env.set_byte_array_region(&byte_array, 0, unsafe {
                std::mem::transmute::<&[u8], &[i8]>(&dng_data)
            }) {
                error!("Failed to transmute DNG data: {:?}", e);
                return std::ptr::null_mut();
            }

            // Create Java string for filename
            let filename_jstr = match env.new_string(&filename) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to create filename string: {:?}", e);
                    return std::ptr::null_mut();
                }
            };

            // Create SaveDng object
            let saved_data_class = match env.find_class("com/lumis/camera/SaveDng") {
                Ok(class) => class,
                Err(e) => {
                    error!("Failed to find SaveDng class: {:?}", e);
                    return std::ptr::null_mut();
                }
            };

            let saved_data_obj = match env.new_object(
                saved_data_class,
                "([BLjava/lang/String;)V", // Constructor signature: (byte[], String)
                &[
                    jni::objects::JValue::Object(&byte_array),
                    jni::objects::JValue::Object(&filename_jstr),
                ],
            ) {
                Ok(obj) => obj,
                Err(e) => {
                    error!("Failed to create SaveDng object: {:?}", e);
                    return std::ptr::null_mut();
                }
            };

            saved_data_obj.into_raw()
        }
        None => {
            // No pending save data
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_CameraInterface_nativeClearSaveInProgress<'local>(
    _env: JNIEnv<'local>,
    _this: JObject<'local>,
    ptr: jlong,
    wrote: jboolean,
) {
    let integrator = integrator_ptr(ptr);

    // Clear both CURRENTLY_SAVING and MANUAL_SAVE bits. Only bump the saved counter when a NEW file was
    // actually written - a dedup skip (filename already on disk) still shows the green save indicator but
    // must NOT increment the counter, since nothing new was saved.
    integrator.header[FLAGS_IDX] &= !(CURRENTLY_SAVING | MANUAL_SAVE_BIT);
    if wrote != 0 {
        integrator.header[SAVED_COUNTER_IDX] += 1;
    }

    if crate::DEBUG {
        info!(
            "Save completed, cleared CURRENTLY_SAVING flag and incremented saved counter to {}",
            integrator.header[SAVED_COUNTER_IDX]
        );
    }
}
