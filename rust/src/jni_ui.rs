use crate::calibration::chameleon::set_android_config_dir;
use crate::shared_memory::*;
use jni::{
    objects::{JClass, JObject, JString},
    sys::{jboolean, jfloat, jint, jlong},
    JNIEnv,
};
use log::*;
use ndk::native_window::NativeWindow;

/// Set the Android config directory for calibration data storage.
/// Must be called before any calibration functions are used.
#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_UserInterface_nativeSetConfigDir(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    config_dir: JString<'_>,
) {
    let Ok(path) = env.get_string(&config_dir) else {
        error!("Failed to get config dir string from JNI");
        return;
    };
    let path_str = path.to_string_lossy();
    set_android_config_dir(&path_str);
}

/// Tag a native window's buffers as Rec.2020 (BT.2020 primaries, sRGB transfer, full
/// range). ANativeWindow_setBuffersDataSpace is API 28+; we link at api-21, so resolve it
/// at runtime via dlsym and skip silently if unavailable (older devices stay sRGB).
type SetDataSpaceFn = unsafe extern "C" fn(*mut ndk_sys::ANativeWindow, i32) -> i32;
// Resolved once: Some(fn) if available, None on older devices. usize holds the fn ptr so
// it's Send/Sync for the OnceLock.
static SET_DATASPACE_FN: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

fn set_buffers_dataspace_bt2020(window: *mut ndk_sys::ANativeWindow) {
    // STANDARD_BT2020 | TRANSFER_GAMMA2_2 | RANGE_FULL. Android has no gamma-2.0 transfer
    // to match our sqrt encode, so 2.2 (pure power, no sRGB toe) is the closest tag.
    const BT2020_GAMMA22_FULL: i32 = 151388160;
    let addr = *SET_DATASPACE_FN.get_or_init(|| unsafe {
        // The symbol lives in libnativewindow.so (API 28+); resolve via an explicit handle
        // since Android's RTLD_DEFAULT isn't exposed through libc.
        let lib = b"libnativewindow.so\0";
        let handle = libc::dlopen(lib.as_ptr() as *const libc::c_char, libc::RTLD_NOW);
        if handle.is_null() {
            return 0;
        }
        let name = b"ANativeWindow_setBuffersDataSpace\0";
        libc::dlsym(handle, name.as_ptr() as *const libc::c_char) as usize
    });
    if addr != 0 {
        unsafe {
            let f: SetDataSpaceFn = std::mem::transmute(addr);
            f(window, BT2020_GAMMA22_FULL);
        }
    }
}

fn ui_context_ptr(ptr: jlong) -> Option<&'static mut crate::ui::ui::UserInterface> {
    if crate::DEBUG && ptr == 0 {
        panic!("Null UI context pointer received");
    }
    unsafe { Some(&mut *(ptr as *mut crate::ui::ui::UserInterface)) }
}

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_UserInterface_nativeUIInit(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    shared_memory_fd: jint,
    shared_memory_size: jlong,
    _surface: JObject<'_>,
    width: jint,
    height: jint,
    _density: jint,
) -> jlong {
    if crate::DEBUG {
        info!(
            "Initializing UserInterface: {}x{}, SharedMemory fd={}, size={}",
            width, height, shared_memory_fd, shared_memory_size
        );
    }

    // Map the shared memory from file descriptor
    let shared_memory = SharedMemory::from_ashmem_fd(shared_memory_fd, shared_memory_size as usize);

    let user_interface = Box::new(crate::ui::ui::UserInterface::from_shared_memory_object(
        width as u32,
        height as u32,
        shared_memory,
    ));

    let ui_context_ptr = Box::into_raw(user_interface) as jlong;
    if crate::DEBUG {
        info!("UserInterface created at 0x{:x}", ui_context_ptr as u64);
    }
    ui_context_ptr
}

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_UserInterface_nativeUIDraw(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    ui_ptr: jlong,
    surface: JObject<'_>,
    complete_exposure: jboolean,
    save: jboolean,
    continuous_save: jboolean,
    gravity_x: jfloat,
    gravity_y: jfloat,
    gravity_z: jfloat,
    touch_x: jfloat,
    touch_y: jfloat,
) {
    let Some(user_interface) = ui_context_ptr(ui_ptr) else {
        error!("Invalid UI context pointer in nativeUIDraw");
        return;
    };

    // Convert Surface to NativeWindow
    let Some(window) = (unsafe { NativeWindow::from_surface(env.get_raw(), surface.as_raw()) })
    else {
        error!("Failed to convert Surface to NativeWindow");
        return;
    };

    // Tag the surface as Rec.2020 so SurfaceFlinger does the final 2020 -> panel
    // conversion. We render Rec.2020 primaries (via magic_9_display) with a sqrt (~sRGB)
    // transfer, full range. STANDARD_BT2020 | TRANSFER_SRGB | RANGE_FULL = 142999040.
    // ANativeWindow_setBuffersDataSpace needs API 28, but we link against api-21, so we
    // resolve it at runtime via dlsym (no-op on older devices). Idempotent per frame.
    set_buffers_dataspace_bt2020(window.ptr().as_ptr());

    // Set flags in SharedMemory based on button states
    if complete_exposure != 0 {
        user_interface.set_shared_memory_flag(COMPLETE_EXPOSURE_BIT);
    }

    // Handle save logic - volume up should cancel continuous save if active
    if save != 0 {
        let flags = user_interface.header[FLAGS_IDX];
        if (flags & CONTINUOUS_SAVE_BIT) != 0 {
            // Continuous save is active - cancel it (clear the bit)
            user_interface.header[FLAGS_IDX] &= !CONTINUOUS_SAVE_BIT;
            if crate::DEBUG {
                log::info!("Volume up pressed - cancelling continuous save");
            }
        } else {
            // Continuous save not active - trigger single save
            user_interface.set_shared_memory_flag(MANUAL_SAVE_BIT);
            if crate::DEBUG {
                log::info!("Volume up pressed - triggering single save");
            }
        }
    }

    if continuous_save != 0 {
        user_interface.set_shared_memory_flag(CONTINUOUS_SAVE_BIT);
        if crate::DEBUG {
            log::info!("Volume down pressed - starting continuous save");
        }
    }

    // Handle input and draw - UserInterface does everything internally
    user_interface
        .handle_input_and_draw(&window, touch_x, touch_y, gravity_x, gravity_y, gravity_z);
}
