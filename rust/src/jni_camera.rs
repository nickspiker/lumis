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

    let integrator_ptr = Box::into_raw(integrator) as jlong;
    if crate::DEBUG {
        info!("Camera integrator created at 0x{:x}", integrator_ptr as u64);
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
) {
    let integrator = integrator_ptr(ptr);

    // Clear both CURRENTLY_SAVING and MANUAL_SAVE bits, increment saved counter
    integrator.header[FLAGS_IDX] &= !(CURRENTLY_SAVING | MANUAL_SAVE_BIT);
    integrator.header[SAVED_COUNTER_IDX] += 1;

    if crate::DEBUG {
        info!(
            "Save completed, cleared CURRENTLY_SAVING flag and incremented saved counter to {}",
            integrator.header[SAVED_COUNTER_IDX]
        );
    }
}
