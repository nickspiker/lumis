// mod debayer;
// Calibration now comes from the `chameleon` crate (see Cargo.toml), not a local copy.
mod image;
mod jni_camera;
mod jni_menu;
mod jni_ui;
mod sensor_interface;
mod shared_memory;
mod ui;
mod ui_main_menu;

pub const DEBUG: bool = false;

// Global JavaVM pointer for JNI callbacks from background threads
static mut JAVA_VM: *mut jni::sys::JavaVM = std::ptr::null_mut();

#[cfg(feature = "jni")]
#[no_mangle]
pub extern "system" fn JNI_OnLoad(vm: jni::JavaVM, _: *mut std::os::raw::c_void) -> jni::sys::jint {
    // Store JavaVM pointer for later use
    unsafe {
        JAVA_VM = vm.get_java_vm_pointer();
    }
    // Get process name to determine which process we're in
    let pid = std::process::id();
    let cmdline_path = format!("/proc/{}/cmdline", pid);
    let process_name =
        std::fs::read_to_string(&cmdline_path).unwrap_or_else(|_| "unknown".to_string());

    let log_tag = if process_name.contains(":camera") {
        "lumis_camera"
    } else {
        "lumis_ui"
    };

    // Initialize Android logger with process-specific tag
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag(log_tag)
            .with_max_level(log::LevelFilter::Debug),
    );

    // Set panic hook with process identification
    let process_type = if process_name.contains(":camera") {
        "CAMERA"
    } else {
        "UI"
    };

    std::panic::set_hook(Box::new(move |panic_info| {
        log::error!("{} PROCESS PANIC: {}", process_type, panic_info);
        if let Some(location) = panic_info.location() {
            log::error!("PANIC location: {}:{}", location.file(), location.line());
        }
        if let Some(msg) = panic_info.payload().downcast_ref::<&str>() {
            log::error!("PANIC message: {}", msg);
        }
        if let Some(msg) = panic_info.payload().downcast_ref::<String>() {
            log::error!("PANIC message: {}", msg);
        }
    }));

    log::info!(
        "Lumis JNI loaded in {} process (PID: {})",
        process_type,
        pid
    );
    jni::sys::JNI_VERSION_1_6
}

pub use image::integrator::*;
pub use sensor_interface::CameraInfo;
pub use ui::ui::UserInterface;

#[derive(Clone, Debug)]
pub enum AppState {
    Menu = 0,
    Camera = 1,
}

#[cfg(feature = "jni")]
pub use jni_camera::*;
pub use jni_menu::*;
