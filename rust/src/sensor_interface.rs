use log::*;
#[derive(Clone)]
pub struct CameraInfo {
    pub id: String,   // Android camera ID (e.g., "0", "1", "2")
    pub index: usize, // Array index for selection
    pub facing: i32,
    pub width: u32,
    pub height: u32,
    pub white_level: i32,
    pub black_level: i32,
    pub bayer_pattern: i32,
    pub supports_raw: bool,
    pub min_iso: i32,
    pub max_iso: i32,
    pub min_exposure: i64,
    pub max_exposure: i64,
    pub sensor_width: f32,
    pub sensor_height: f32,
    pub focal_lengths: Vec<f32>,
    pub apertures: Vec<f32>,
    pub min_focus_distance: f32,
    pub has_ois: bool,
    pub hardware_level: i32,
    pub sensor_orientation: i32,
    pub pixel_array_width: u32,
    pub mode_count: u32, // >1 on group head => this lens has multiple capture modes
    pub group_id: i32,   // modes of the same physical lens share this (-1 = ungrouped)
}

pub fn parse_camera_array(data: &[f32]) -> Vec<CameraInfo> {
    if crate::DEBUG {
        debug!("Parsing camera array: {} elements", data.len());
    }

    let mut cameras = Vec::new();
    let mut i = 0;
    let mut camera_count = 0;

    while i < data.len() {
        if data[i].is_nan() {
            if crate::DEBUG {
                debug!("Found camera delimiter (NaN) at index {}", i);
            }
            camera_count += 1;

            if let Some(camera) = parse_single_camera(&data[i..]) {
                if crate::DEBUG {
                    info!(
                        "Successfully parsed camera {}: id='{}', {}x{}",
                        camera_count, camera.id, camera.width, camera.height
                    );
                    debug!(
                        "Camera {} details: facing={}, raw={}, ISO={}-{}, exposure={}-{}ns",
                        camera_count,
                        camera.facing,
                        camera.supports_raw,
                        camera.min_iso,
                        camera.max_iso,
                        camera.min_exposure,
                        camera.max_exposure
                    );
                }
                cameras.push(camera);
            } else {
                if crate::DEBUG {
                    warn!(
                        "Failed to parse camera {} starting at index {}",
                        camera_count, i
                    );
                    debug!(
                        "Available data from index {}: {} elements",
                        i,
                        data.len() - i
                    );
                }
            }

            i += 1;
            // Skip to next camera delimiter (-0.0). Detect by sign bit: -0.0 == 0.0 by
            // IEEE equality, so `!= -0.0` would also stop at a legitimate 0.0 field
            // (e.g. groupId 0). Only the true negative-zero terminator should end the skip.
            let start_skip = i;
            while i < data.len() && !(data[i] == 0.0 && data[i].is_sign_negative()) {
                i += 1;
            }
            if crate::DEBUG {
                debug!("Skipped {} elements to find next camera", i - start_skip);
            }
            i += 1;
        } else {
            i += 1;
        }
    }

    if crate::DEBUG {
        info!(
            "Camera array parsing complete: {} cameras found",
            cameras.len()
        );
    }
    cameras
}

pub fn parse_single_camera(data: &[f32]) -> Option<CameraInfo> {
    if data.len() < 25 {
        error!(
            "Insufficient data for camera parsing: {} elements (minimum 25)",
            data.len()
        );
        return None;
    }

    if crate::DEBUG {
        debug!("Parsing single camera from {} elements", data.len());
    }

    let mut i = 1; // Skip the NaN delimiter

    let index = data[i] as usize;
    if crate::DEBUG {
        debug!("Camera index: {}", index);
    }
    i += 1;

    // Parse Android camera ID string
    if i >= data.len() {
        error!(
            "Unexpected end of data while reading ID length at index {}",
            i
        );
        return None;
    }
    let id_length = data[i] as usize;
    i += 1;

    if id_length > 64 {
        error!("Suspiciously long camera ID: {} characters", id_length);
        return None;
    }

    if i + id_length > data.len() {
        error!(
            "ID length {} extends beyond available data (need {} more elements)",
            id_length,
            (i + id_length) - data.len()
        );
        return None;
    }

    let mut camera_id = String::with_capacity(id_length);
    for j in 0..id_length {
        let char_code = data[i] as u8;
        if !char_code.is_ascii() {
            warn!(
                "Non-ASCII character in camera ID at position {}: {}",
                j, char_code
            );
        }
        camera_id.push(char_code as char);
        i += 1;
    }
    if crate::DEBUG {
        debug!("Camera ID: '{}'", camera_id);
    }

    if i + 20 > data.len() {
        error!(
            "Insufficient data for basic camera properties (need {} more elements)",
            (i + 20) - data.len()
        );
        return None;
    }

    let facing = data[i] as i32;
    i += 1;
    let width = data[i] as u32;
    i += 1;
    let height = data[i] as u32;
    i += 1;
    let white_level = data[i] as i32;
    i += 1;
    let black_level = data[i] as i32;
    i += 1;
    let bayer_pattern = data[i] as i32;
    i += 1;

    if crate::DEBUG {
        debug!(
            "Basic properties: {}x{}, facing={}, white={}, black={}, bayer={}",
            width, height, facing, white_level, black_level, bayer_pattern
        );
    }

    let supports_raw = if data[i].is_infinite() {
        let raw_support = data[i] > 0.0;
        if crate::DEBUG {
            debug!(
                "RAW support: {} (from infinite value {})",
                raw_support, data[i]
            );
        }
        raw_support
    } else {
        if crate::DEBUG {
            debug!("RAW support: false (non-infinite value {})", data[i]);
        }
        false
    };
    i += 1;

    let min_iso = data[i] as i32;
    i += 1;
    let max_iso = data[i] as i32;
    i += 1;
    let min_exposure = data[i] as i64;
    i += 1;
    let max_exposure = data[i] as i64;
    i += 1;
    let sensor_width = data[i];
    i += 1;
    let sensor_height = data[i];
    i += 1;

    if crate::DEBUG {
        debug!(
            "Capabilities: ISO {}-{}, exposure {}-{}ns, sensor {:.2}x{:.2}mm",
            min_iso, max_iso, min_exposure, max_exposure, sensor_width, sensor_height
        );
    }

    if i >= data.len() {
        error!(
            "Unexpected end of data while reading focal length count at index {}",
            i
        );
        return None;
    }

    let focal_length_count = data[i] as usize;
    i += 1;

    if focal_length_count > 16 {
        error!(
            "Suspiciously high focal length count: {}",
            focal_length_count
        );
        return None;
    }

    if i + focal_length_count > data.len() {
        error!(
            "Focal length array extends beyond available data (need {} more elements)",
            (i + focal_length_count) - data.len()
        );
        return None;
    }

    let mut focal_lengths = Vec::with_capacity(focal_length_count);
    for j in 0..focal_length_count {
        focal_lengths.push(data[i]);
        if crate::DEBUG {
            trace!("Focal length {}: {:.2}mm", j, data[i]);
        }
        i += 1;
    }
    if crate::DEBUG {
        debug!("Focal lengths: {:?}", focal_lengths);
    }

    if i >= data.len() {
        error!(
            "Unexpected end of data while reading aperture count at index {}",
            i
        );
        return None;
    }

    let aperture_count = data[i] as usize;
    i += 1;

    if aperture_count > 16 {
        error!("Suspiciously high aperture count: {}", aperture_count);
        return None;
    }

    if i + aperture_count > data.len() {
        error!(
            "Aperture array extends beyond available data (need {} more elements)",
            (i + aperture_count) - data.len()
        );
        return None;
    }

    let mut apertures = Vec::with_capacity(aperture_count);
    for j in 0..aperture_count {
        apertures.push(data[i]);
        if crate::DEBUG {
            trace!("Aperture {}: f/{:.1}", j, data[i]);
        }
        i += 1;
    }
    if crate::DEBUG {
        debug!("Apertures: {:?}", apertures);
    }

    if i + 5 > data.len() {
        error!(
            "Insufficient data for final camera properties (need {} more elements)",
            (i + 5) - data.len()
        );
        return None;
    }

    let min_focus_distance = data[i];
    i += 1;

    let has_ois = if data[i].is_infinite() {
        let ois_available = data[i] > 0.0;
        if crate::DEBUG {
            debug!(
                "OIS available: {} (from infinite value {})",
                ois_available, data[i]
            );
        }
        ois_available
    } else {
        if crate::DEBUG {
            debug!("OIS available: false (non-infinite value {})", data[i]);
        }
        false
    };
    i += 1;

    let hardware_level = data[i] as i32;
    i += 1;
    let sensor_orientation = data[i] as i32;
    i += 1;
    let pixel_array_width = data[i] as u32;
    i += 1;

    // mode_count is appended after pixel_array_width; default to 1 if absent.
    let mode_count = if i < data.len() && !data[i].is_nan() && data[i] != -0.0 {
        (data[i] as u32).max(1)
    } else {
        1
    };
    i += 1;

    // group_id follows mode_count; default to -1 (ungrouped) if absent. NOTE: the
    // record terminator is -0.0, which == 0.0 by IEEE equality, so we must detect it by
    // sign bit (is_sign_negative) - otherwise a legitimate groupId 0 looks like the end.
    let is_terminator = |v: f32| v.is_nan() || (v == 0.0 && v.is_sign_negative());
    let group_id = if i < data.len() && !is_terminator(data[i]) {
        data[i] as i32
    } else {
        -1
    };

    if crate::DEBUG {
        debug!(
            "Final properties: focus={:.2}, OIS={}, hw_level={}, orientation={}°, pixel_width={}, modes={}",
            min_focus_distance, has_ois, hardware_level, sensor_orientation, pixel_array_width, mode_count
        );
    }

    let camera = CameraInfo {
        id: camera_id,
        index,
        facing,
        width,
        height,
        white_level,
        black_level,
        bayer_pattern,
        supports_raw,
        min_iso,
        max_iso,
        min_exposure,
        max_exposure,
        sensor_width,
        sensor_height,
        focal_lengths,
        apertures,
        min_focus_distance,
        has_ois,
        hardware_level,
        sensor_orientation,
        pixel_array_width,
        mode_count,
        group_id,
    };

    if crate::DEBUG {
        info!(
            "Camera '{}' parsed successfully: {}x{}, {} focal lengths, {} apertures",
            camera.id,
            camera.width,
            camera.height,
            camera.focal_lengths.len(),
            camera.apertures.len()
        );
    }

    Some(camera)
}
