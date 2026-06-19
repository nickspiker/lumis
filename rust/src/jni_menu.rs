use crate::*;
extern crate libc;
use jni::{
    objects::{JClass, JFloatArray, JObject},
    sys::{jboolean, jfloat, jint, jlong},
    JNIEnv,
};
use log::*;
use sensor_interface::parse_camera_array;

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_UserInterface_nativeMenuInit<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    width: jint,
    height: jint,
    density: jint,
    camera_array: JObject<'local>,
) -> jlong {
    if crate::DEBUG {
        info!(
            "Initializing main menu: {}x{} @ {}dpi with cameras",
            width, height, density
        );
    }

    // Create main menu - pure camera selection UI
    let mut menu = Box::new(ui_main_menu::MainMenu::new(
        width as u32,
        height as u32,
        density as f32,
    ));

    // Parse and set camera list immediately
    let camera_array = JFloatArray::from(camera_array);
    match env.get_array_length(&camera_array) {
        Ok(len) => {
            let mut buffer = vec![0.0f32; len as usize];
            if let Err(e) = env.get_float_array_region(&camera_array, 0, &mut buffer) {
                error!("Failed to read camera array: {}", e);
            } else {
                let cameras = parse_camera_array(&buffer);
                let camera_count = cameras.len();
                menu.set_camera_list(cameras);
                if crate::DEBUG {
                    info!("Menu initialized with {} cameras", camera_count);
                }
            }
        }
        Err(e) => {
            error!("Failed to get camera array length: {}", e);
        }
    }

    let menu_ptr = Box::into_raw(menu) as jlong;
    if crate::DEBUG {
        info!("Main menu created at 0x{:x}", menu_ptr as u64);
    }
    menu_ptr
}

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_UserInterface_nativeMenuHandleTouch<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    menu_ptr: jlong,
    action: jint,
    x: jfloat,
    y: jfloat,
) -> jni::objects::JIntArray<'local> {
    if crate::DEBUG {
        debug!(
            "Menu touch event: action={}, pos=({:.1}, {:.1})",
            action, x, y
        );
    }

    let menu = unsafe { &mut *(menu_ptr as *mut ui_main_menu::MainMenu) };

    let (menu_action, needs_render) = menu.handle_touch(action, x, y);

    // Prepare return values
    let mut needs_redraw = 0i32;
    let mut selected_camera = -1i32;

    if needs_render {
        needs_redraw = 1;
    }

    if let Some(action) = menu_action {
        match action {
            ui_main_menu::MenuAction::StartCamera(camera_index) => {
                if crate::DEBUG {
                    info!("Menu action: Camera {} selected", camera_index);
                }
                selected_camera = camera_index as i32;
            }
            ui_main_menu::MenuAction::Exit => {
                if crate::DEBUG {
                    info!("Menu action: Exit requested");
                }
                unsafe {
                    libc::exit(0);
                }
            }
        }
    }

    // Return [needs_redraw, selected_camera] as int array
    match _env.new_int_array(2) {
        Ok(array) => {
            let result_data = [needs_redraw, selected_camera];
            if let Err(e) = _env.set_int_array_region(&array, 0, &result_data) {
                error!("Failed to set menu touch result: {}", e);
            }
            array
        }
        Err(_) => {
            error!("Failed to create menu touch result array");
            jni::objects::JIntArray::from(jni::objects::JObject::null())
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_com_lumis_camera_UserInterface_nativeMenuDraw(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    menu_ptr: jlong,
    surface: JObject<'_>,
    full_draw: jboolean,
) {
    let menu = unsafe { &mut *(menu_ptr as *mut ui_main_menu::MainMenu) };

    unsafe {
        if let Some(native_window) = ndk::native_window::NativeWindow::from_surface(
            _env.get_native_interface(),
            surface.into_raw(),
        ) {
            menu.draw(&native_window, full_draw != 0);
        } else {
            error!("Failed to get native window from surface in menu draw");
        }
    }
}
