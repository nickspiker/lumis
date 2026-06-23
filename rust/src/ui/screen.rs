use crate::shared_memory::*;
use crate::ui::{
    controls::*,
    histogram::draw_histogram_overlay,
    text::TextRenderer,
    ui::{ui_constants, UserInterface},
};
use ndk::native_window::NativeWindow;
use ndk_sys::{ANativeWindow_Buffer, ANativeWindow_lock, ANativeWindow_unlockAndPost};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;

/// Apply the 3x3 camera->Rec.2020 display matrix (magic_9_display) to a linear RGB
/// triple. Row-major: out_r = m0*r + m1*g + m2*b, etc. Done in linear light, before the
/// sqrt display encode.
#[inline]
fn apply_display_matrix(ui: &UserInterface, r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let m = &ui.magic_9_display;
    (
        m[0] * r + m[1] * g + m[2] * b,
        m[3] * r + m[4] * g + m[5] * b,
        m[6] * r + m[7] * g + m[8] * b,
    )
}

pub fn draw_screen(ui: &mut UserInterface, window: &NativeWindow, mut full_draw: bool) {
    // Check for new image by comparing counters - determines full vs partial draw
    let current_image_counter = ui.header[IMAGE_COUNTER_IDX];

    // While the feed is frozen for calibration (and the overlay held), force full draws: the
    // image counter is frozen so the normal new-frame trigger won't fire, and we need the
    // frozen frame + overlay to paint, then a clean repaint on dismiss.
    let hold_present = ui.calibration_hold.load().is_some() || ui.frozen_image_counter.is_some();
    if hold_present || ui.previous_hold_present {
        full_draw = true;
    }
    ui.previous_hold_present = hold_present;

    if ui.histogram_visible && ui.histogram_buffer.load().is_some() {
        // Histogram mode with histogram ready - use histogram counter for full draw decisions
        let current_histogram_counter = ui.header[HISTOGRAM_COUNTER_IDX];
        full_draw |= current_histogram_counter != ui.previous_histogram_counter;
        ui.previous_histogram_counter = current_histogram_counter;
    } else {
        // Normal mode OR histogram visible but not ready - use image/exposure counters
        full_draw |= current_image_counter != ui.previous_image_counter;
        let current_start = ui.header[EXPOSURE_START_SECS_IDX];
        full_draw |= current_start != ui.previous_exposure_start;
        ui.previous_exposure_start = current_start;
    }

    // Check if we need to calculate a new histogram (only when image counter changes AND not currently calculating)
    let image_counter_changed = current_image_counter != ui.previous_image_counter;
    if ui.histogram_visible
        && image_counter_changed
        && !ui.calculating_histogram.load(Ordering::Acquire)
    {
        spawn_histogram_calculation(ui, current_image_counter);
    }

    // Update image counter tracking
    ui.previous_image_counter = current_image_counter;

    // Image counter tracking moved above for histogram/normal mode split

    unsafe {
        let mut buffer = std::mem::zeroed::<ANativeWindow_Buffer>();

        if ANativeWindow_lock(window.ptr().as_ptr(), &mut buffer, std::ptr::null_mut()) < 0 {
            return;
        }

        let pixels = std::slice::from_raw_parts_mut(
            buffer.bits as *mut u8,
            (buffer.stride * buffer.height * 3) as usize,
        );

        if full_draw {
            let value = ((ui.magic_counter[0] as u32) << 16)
                | ((ui.magic_counter[1] as u32) << 8)
                | (ui.magic_counter[2] as u32);
            let new_value = (value + 1) & 0xFFFFFF;
            ui.magic_counter[0] = (new_value >> 16) as u8;
            ui.magic_counter[1] = (new_value >> 8) as u8;
            ui.magic_counter[2] = new_value as u8;

            draw_full_screen(ui, pixels, &buffer);

            // Write magic pixel to top-right corner
            let tr_idx = (buffer.stride - 1) as usize * 3;
            pixels[tr_idx] = ui.magic_counter[0]; // Red
            pixels[tr_idx + 1] = ui.magic_counter[1]; // Green
            pixels[tr_idx + 2] = ui.magic_counter[2]; // Blue
        } else {
            // Partial draw attempt
            // Check top-right pixel (stride-1, 0) to see if buffer is still valid
            let tr_idx = (buffer.stride - 1) as usize * 3;

            // TR pixel check for buffer validation
            if pixels[tr_idx + 2] != ui.magic_counter[2]
                || pixels[tr_idx + 1] != ui.magic_counter[1]
                || pixels[tr_idx] != ui.magic_counter[0]
            {
                // Buffer is stale or was overwritten, do full screen instead (don't increment counter)
                draw_full_screen(ui, pixels, &buffer);
                let tr_idx = (buffer.stride - 1) as usize * 3;
                pixels[tr_idx] = ui.magic_counter[0]; // Red
                pixels[tr_idx + 1] = ui.magic_counter[1]; // Green
                pixels[tr_idx + 2] = ui.magic_counter[2]; // Blue
            } else {
                if ui.controls_visible {
                    restore_label_areas(ui, pixels, buffer.stride as usize);
                    restore_counter_areas(ui, pixels, buffer.stride as usize);
                    draw_progress(ui, pixels, &buffer);
                    draw_counters(ui, pixels, &buffer);
                }
            }
        }

        ANativeWindow_unlockAndPost(window.ptr().as_ptr());
    }
}

fn spawn_histogram_calculation(ui: &UserInterface, image_counter: u64) {
    ui.calculating_histogram.store(true, Ordering::Release);

    // Clone the raw data directly from shared memory
    let current_slot = (image_counter & 3) as usize;
    let pixel_count = ui.sensor_x_size * ui.sensor_y_size;
    let avg_offset = current_slot * pixel_count; // Left half is average
    let raw_data: Vec<u16> = ui.image_buffer[avg_offset..avg_offset + pixel_count].to_vec();

    // Clone only what the thread needs
    let width = ui.sensor_x_size as usize;
    let height = ui.sensor_y_size as usize;
    let black_level = ui.raw_black_level;
    let bayer_pattern = ui.bayer_pattern;
    let screen_width = ui.screen_run as usize;
    let screen_height = ui.screen_rise as usize;
    let controls_visible = ui.controls_visible;
    let rotation = ui.device_rotation;

    // Clone Arc references for the thread
    let histogram_buffer = ui.histogram_buffer.clone();
    let calculating_histogram = ui.calculating_histogram.clone();

    // Get raw pointer to shared memory header for atomic counter increment (cast to usize for Send)
    let header_addr = ui.header.as_ptr() as usize;

    thread::spawn(move || {
        let mut text_renderer = TextRenderer::new();

        // Calculate histogram
        let mut local_buffer = vec![0u8; screen_width * screen_height * 3];

        let bayer_array = match bayer_pattern {
            0 => [0, 1, 1, 2], // RGGB: R G / G B
            1 => [1, 0, 2, 1], // GRBG: G R / B G
            2 => [1, 2, 0, 1], // GBRG: G B / R G
            3 => [2, 1, 1, 0], // BGGR: B G / G R
            _ => panic!("Unknown bayer pattern: {}", bayer_pattern),
        };

        draw_histogram_overlay(
            &raw_data,
            width,
            height,
            black_level,
            bayer_array,
            &mut local_buffer,
            screen_width,
            screen_height,
            controls_visible,
            rotation,
            &mut text_renderer,
        );

        // Atomically update histogram buffer
        histogram_buffer.store(Arc::new(Some(local_buffer)));

        // Increment histogram counter in shared memory to trigger full draw
        unsafe {
            let header_ptr = header_addr as *mut u64;
            let header_slice =
                std::slice::from_raw_parts_mut(header_ptr, crate::shared_memory::IMAGE_START);
            header_slice[crate::shared_memory::HISTOGRAM_COUNTER_IDX] += 1;
        }

        // Clear calculating flag
        calculating_histogram.store(false, Ordering::Release);
    });
}

fn draw_full_screen(ui: &mut UserInterface, pixels: &mut [u8], buffer: &ANativeWindow_Buffer) {
    // Draw the camera image (frozen during calibration). When a calibration hold is active,
    // the in-place live overlay is composited per-pixel inside the fit-to-screen loop.
    draw_camera_or_histogram(ui, pixels, buffer);

    if ui.controls_visible {
        draw_controls(ui, pixels, buffer);
        save_counter_areas(ui, pixels, buffer.stride as usize);
        draw_counters(ui, pixels, buffer);
        draw_progress(ui, pixels, buffer);
    }

    if crate::DEBUG {
        debug_draw_margins(ui, pixels, buffer);
    }
}

// The original image-drawing body: histogram if visible, else the camera image.
fn draw_camera_or_histogram(ui: &mut UserInterface, pixels: &mut [u8], buffer: &ANativeWindow_Buffer) {
    // Draw histogram if visible and available, otherwise draw image
    let draw_histogram = ui.histogram_visible && ui.histogram_buffer.load().is_some();

    if draw_histogram {
        let histogram_data = ui.histogram_buffer.load();
        if let Some(ref histogram) = **histogram_data {
            for y in 0..ui.screen_rise {
                for x in 0..ui.screen_run {
                    let src_idx = (y * ui.screen_run + x) * 3;
                    let dst_idx = (y * buffer.stride as usize + x) * 3;

                    pixels[dst_idx] = histogram[src_idx];
                    pixels[dst_idx + 1] = histogram[src_idx + 1];
                    pixels[dst_idx + 2] = histogram[src_idx + 2];
                }
            }
        }
    } else {
        let pixel_count = ui.sensor_x_size * ui.sensor_y_size;

        // While calibration holds the feed, render from the snapshot taken at freeze time
        // (avg at 0, diff at pixel_count) so the camera thread can't change it. Otherwise
        // read the current live slot.
        let frozen = ui.frozen_image_counter.is_some() && ui.frozen_image.len() >= 2 * pixel_count;
        let (raw_average, raw_difference) = if frozen {
            (
                &ui.frozen_image[0..pixel_count],
                &ui.frozen_image[pixel_count..2 * pixel_count],
            )
        } else {
            let current_slot = (ui.header[IMAGE_COUNTER_IDX] & 3) as usize;
            let avg_offset = (current_slot * 2) * pixel_count;
            let diff_offset = (current_slot * 2 + 1) * pixel_count;
            (
                &ui.image_buffer[avg_offset..avg_offset + pixel_count],
                &ui.image_buffer[diff_offset..diff_offset + pixel_count],
            )
        };

        // Select data source based on current mode
        let current_mode = RawMode::from(ui.header[CURRENT_MODE_IDX] as u8);
        // Determine if we need to swap dimensions based on rotation
        let needs_rotation = ui.sensor_orientation == 90 || ui.sensor_orientation == 270;
        let (effective_width, effective_height) = if needs_rotation {
            (ui.sensor_y_size, ui.sensor_x_size)
        } else {
            (ui.sensor_x_size, ui.sensor_y_size)
        };

        if ui.view_1to1 {
            // 1:1 pixel view (zoomed in). Each screen pixel maps to one sensor pixel via the pan
            // offset + rotation. Average mode is colour: cheap per-pixel 2x2-block debayer +
            // display matrix + sqrt (fast enough for the per-frame UI thread — the high-quality
            // RCD demosaic is reserved for saving, not the live preview, matching how camera apps
            // run a light demosaic live and the good one only on capture). Difference/Motion are
            // monochrome magnitudes, left unmixed.
            let scale_avg =
                ui.display_gain as f32 * (65536. / (65536. - ui.raw_black_level as f32));
            for y in 0..ui.screen_rise {
                for x in 0..ui.screen_run {
                    let dst_idx = (y * buffer.stride as usize + x) * 3;

                    // Calculate sensor coordinates with pan offset
                    let display_x = ((x as f32 + ui.pan_offset_x) as isize) as usize;
                    let display_y = ((y as f32 + ui.pan_offset_y) as isize) as usize;

                    // Apply rotation transform to get native sensor coordinates
                    let (sensor_x, sensor_y) = match (ui.sensor_orientation, ui.camera_facing) {
                        (90, 0) => {
                            // Front camera, 90°: horizontal mirror of back camera 90°
                            (ui.sensor_x_size - display_y, display_x)
                        }
                        (270, 0) => {
                            // Front camera, 270°: tested working!
                            (ui.sensor_x_size - display_y, ui.sensor_y_size - display_x)
                        }
                        (90, 1) => {
                            // Back camera, 90°: tested working!
                            (display_y, ui.sensor_y_size - display_x)
                        }
                        (270, 1) => {
                            // Back camera, 270°: no horizontal mirror
                            (display_y, display_x)
                        }
                        (0, 0) => {
                            // Front camera, 0°: no rotation + horizontal mirror
                            (ui.sensor_x_size - display_x, display_y)
                        }
                        (0, 1) => {
                            // Back camera, 0°: no rotation, no mirror
                            (display_x, display_y)
                        }
                        (180, 0) => {
                            // Front camera, 180°: 180° rotation + horizontal mirror
                            (display_x, ui.sensor_y_size - display_y)
                        }
                        (180, 1) => {
                            // Back camera, 180°: 180° rotation, no mirror
                            (ui.sensor_x_size - display_x, ui.sensor_y_size - display_y)
                        }
                        _ => {
                            // Unknown orientation: no rotation, no mirror
                            (display_x, display_y)
                        }
                    };

                    // Draw black outside RAW area
                    if sensor_x >= ui.sensor_x_size || sensor_y >= ui.sensor_y_size {
                        pixels[dst_idx] = 0;
                        pixels[dst_idx + 1] = 0;
                        pixels[dst_idx + 2] = 0;
                        continue;
                    }

                    let idx = sensor_y * ui.sensor_x_size + sensor_x;
                    match current_mode {
                        RawMode::Average => {
                            // 2x2-block colour debayer at (sensor_x, sensor_y), same scheme as the
                            // fit-to-screen preview and the save fallback.
                            let bx = sensor_x & !1;
                            let by = sensor_y & !1;
                            let last = ui.sensor_x_size * ui.sensor_y_size - 1;
                            let base = (by * ui.sensor_x_size + bx).min(last);
                            // Black-level subtract, mirroring the fit-to-screen path EXACTLY so
                            // both clip indicators match: controls visible -> white-clip zeros
                            // over-threshold channels (-> dark/false colour) AND black-clip uses a
                            // WRAPPING sub so a pixel below black_level underflows to a huge u16 ->
                            // renders WHITE (the crushed-shadow indicator). Controls hidden ->
                            // saturating sub (no overlay).
                            let bk = ui.raw_black_level;
                            let sub = |v: u16| -> f32 {
                                if ui.controls_visible {
                                    if v > ui_constants::CLIPPING_THRESHOLD {
                                        0.
                                    } else {
                                        v.wrapping_sub(bk) as f32
                                    }
                                } else {
                                    v.saturating_sub(bk) as f32
                                }
                            };
                            let tl = sub(raw_average[base]);
                            let tr = sub(raw_average[(base + 1).min(last)]);
                            let bl = sub(raw_average[(base + ui.sensor_x_size).min(last)]);
                            let br = sub(raw_average[(base + ui.sensor_x_size + 1).min(last)]);
                            let local = (sensor_x - bx) + (sensor_y - by);
                            let (r, g, b) = match ui.bayer_pattern {
                                0 => (tl, if local < 2 { tr } else { bl }, br), // RGGB
                                1 => (tr, if local < 2 { tl } else { br }, bl), // GRBG
                                2 => (bl, if local < 2 { tl } else { br }, tr), // GBRG
                                3 => (br, if local < 2 { tr } else { bl }, tl), // BGGR
                                _ => (tl, if local < 2 { tr } else { bl }, br),
                            };
                            // Black already subtracted above; just apply gain + matrix.
                            let r = r * scale_avg;
                            let g = g * scale_avg;
                            let b = b * scale_avg;
                            let (lr, lg, lb) = apply_display_matrix(ui, r, g, b);
                            pixels[dst_idx] = (lr.max(0.).sqrt()).min(255.) as u8;
                            pixels[dst_idx + 1] = (lg.max(0.).sqrt()).min(255.) as u8;
                            pixels[dst_idx + 2] = (lb.max(0.).sqrt()).min(255.) as u8;
                        }
                        RawMode::Difference | RawMode::Motion => {
                            let pixel_value = match current_mode {
                                RawMode::Difference => raw_difference[idx],
                                _ => {
                                    let diff_value = raw_difference[idx] as u32;
                                    let avg_value = raw_average[idx] as u32;
                                    let corrected_avg = avg_value
                                        .max(ui.raw_black_level as u32 + 1)
                                        - ui.raw_black_level as u32;
                                    ((diff_value << 16) / corrected_avg).min(65535) as u16
                                }
                            };
                            if ui.controls_visible
                                && pixel_value > ui_constants::CLIPPING_THRESHOLD
                            {
                                pixels[dst_idx] = 0;
                                pixels[dst_idx + 1] = 0;
                                pixels[dst_idx + 2] = 0;
                            } else {
                                let value =
                                    ((pixel_value as f32 * ui.display_gain as f32).sqrt()) as u8;
                                pixels[dst_idx] = value;
                                pixels[dst_idx + 1] = value;
                                pixels[dst_idx + 2] = value;
                            }
                        }
                    }
                }
            }
        } else {
            // In-place calibration overlay (live_overlay): held until tap, composited per
            // sensor pixel below. (min_x, min_y, ov_w, ov_h are full-res sensor coords at 2x
            // scale; rgba_f32 linear 0..1.)
            let cal_hold = ui.calibration_hold.load();
            let cal_overlay = cal_hold.as_ref().as_ref();

            // Normal fit-to-screen view with debayering
            // Calculate scale to fit rotated sensor (if needed) in display while maintaining aspect ratio
            let scale_x = ui.screen_run as f32 / effective_width as f32;
            let scale_y = ui.screen_rise as f32 / effective_height as f32;
            let scale = scale_x.min(scale_y);

            // Calculate displayed image dimensions and offset for centering
            let scaled_width = (effective_width as f32 * scale) as usize;
            let scaled_height = (effective_height as f32 * scale) as usize;
            let offset_x = (ui.screen_run - scaled_width) / 2;
            let offset_y = (ui.screen_rise - scaled_height) / 2;

            // Scan entire display
            for y in 0..ui.screen_rise {
                for x in 0..ui.screen_run {
                    let dst_idx = (y * buffer.stride as usize + x) * 3;

                    // Map display coordinates to effective sensor space
                    let eff_x = ((x as f32 - offset_x as f32) / scale) as i32;
                    let eff_y = ((y as f32 - offset_y as f32) / scale) as i32;

                    // Draw black outside RAW area
                    if eff_x < 0
                        || eff_x >= effective_width as i32
                        || eff_y < 0
                        || eff_y >= effective_height as i32
                    {
                        pixels[dst_idx] = 0;
                        pixels[dst_idx + 1] = 0;
                        pixels[dst_idx + 2] = 0;
                        continue;
                    }

                    let eff_x = eff_x as usize;
                    let eff_y = eff_y as usize;

                    // Apply rotation transformation to get native sensor coordinates
                    let (sensor_x, sensor_y) = match (ui.sensor_orientation, ui.camera_facing) {
                        (90, 0) => {
                            // Front camera, 90°: horizontal mirror of back camera 90°
                            (ui.sensor_x_size - 1 - eff_y, eff_x)
                        }
                        (270, 0) => {
                            // Front camera, 270°: tested working!
                            (ui.sensor_x_size - 1 - eff_y, ui.sensor_y_size - 1 - eff_x)
                        }
                        (90, 1) => {
                            // Back camera, 90°: tested working!
                            (eff_y, ui.sensor_y_size - 1 - eff_x)
                        }
                        (270, 1) => {
                            // Back camera, 270°: no horizontal mirror
                            (eff_y, eff_x)
                        }
                        (0, 0) => {
                            // Front camera, 0°: no rotation + horizontal mirror
                            (ui.sensor_x_size - 1 - eff_x, eff_y)
                        }
                        (0, 1) => {
                            // Back camera, 0°: no rotation, no mirror
                            (eff_x, eff_y)
                        }
                        (180, 0) => {
                            // Front camera, 180°: 180° rotation + horizontal mirror
                            (eff_x, ui.sensor_y_size - 1 - eff_y)
                        }
                        (180, 1) => {
                            // Back camera, 180°: 180° rotation, no mirror
                            (ui.sensor_x_size - 1 - eff_x, ui.sensor_y_size - 1 - eff_y)
                        }
                        _ => {
                            // Unknown orientation: no rotation, no mirror
                            (eff_x, eff_y)
                        }
                    };

                    // Find the 2x2 Bayer block this pixel belongs to
                    let block_x: usize = sensor_x & !1;
                    let block_y: usize = sensor_y & !1;

                    // Read the 2x2 Bayer block from selected data source
                    let idx_base = block_y * ui.sensor_x_size + block_x;

                    // Get the raw values based on mode
                    let (tl, tr, bl, br) = match current_mode {
                        RawMode::Average => (
                            raw_average[idx_base],
                            raw_average[idx_base + 1],
                            raw_average[idx_base + ui.sensor_x_size],
                            raw_average[idx_base + ui.sensor_x_size + 1],
                        ),
                        RawMode::Difference => (
                            raw_difference[idx_base],
                            raw_difference[idx_base + 1],
                            raw_difference[idx_base + ui.sensor_x_size],
                            raw_difference[idx_base + ui.sensor_x_size + 1],
                        ),
                        RawMode::Motion => {
                            // Motion mode: calculate diff/average with black level correction for each pixel
                            let calc_motion = |idx: usize| -> u16 {
                                let diff_value = raw_difference[idx] as u32;
                                let avg_value = raw_average[idx] as u32;
                                let corrected_avg = avg_value.max(ui.raw_black_level as u32 + 1)
                                    - ui.raw_black_level as u32;
                                ((diff_value << 16) / corrected_avg).min(65535) as u16
                            };

                            (
                                calc_motion(idx_base),
                                calc_motion(idx_base + 1),
                                calc_motion(idx_base + ui.sensor_x_size),
                                calc_motion(idx_base + ui.sensor_x_size + 1),
                            )
                        }
                    };

                    // Apply clipping and black level adjustments based on mode
                    let (tl, tr, bl, br) = match current_mode {
                        RawMode::Average => {
                            // Clipping overlay (controls visible): white-clip zeros over-threshold
                            // channels (-> dark/false colour), and black-clip uses a WRAPPING
                            // subtract so a pixel below black_level underflows to a huge value ->
                            // renders WHITE. That flash-white-on-crushed-shadow IS the black-clip
                            // indicator. Controls hidden: plain saturating subtract (no overlay).
                            if ui.controls_visible {
                                (
                                    if tl > ui_constants::CLIPPING_THRESHOLD {
                                        0
                                    } else {
                                        tl.wrapping_sub(ui.raw_black_level)
                                    },
                                    if tr > ui_constants::CLIPPING_THRESHOLD {
                                        0
                                    } else {
                                        tr.wrapping_sub(ui.raw_black_level)
                                    },
                                    if bl > ui_constants::CLIPPING_THRESHOLD {
                                        0
                                    } else {
                                        bl.wrapping_sub(ui.raw_black_level)
                                    },
                                    if br > ui_constants::CLIPPING_THRESHOLD {
                                        0
                                    } else {
                                        br.wrapping_sub(ui.raw_black_level)
                                    },
                                )
                            } else {
                                (
                                    tl.saturating_sub(ui.raw_black_level),
                                    tr.saturating_sub(ui.raw_black_level),
                                    bl.saturating_sub(ui.raw_black_level),
                                    br.saturating_sub(ui.raw_black_level),
                                )
                            }
                        }
                        RawMode::Difference | RawMode::Motion => {
                            // No black level subtraction, optional clipping overlay
                            if ui.controls_visible {
                                (
                                    if tl > ui_constants::CLIPPING_THRESHOLD {
                                        0
                                    } else {
                                        tl
                                    },
                                    if tr > ui_constants::CLIPPING_THRESHOLD {
                                        0
                                    } else {
                                        tr
                                    },
                                    if bl > ui_constants::CLIPPING_THRESHOLD {
                                        0
                                    } else {
                                        bl
                                    },
                                    if br > ui_constants::CLIPPING_THRESHOLD {
                                        0
                                    } else {
                                        br
                                    },
                                )
                            } else {
                                (tl, tr, bl, br)
                            }
                        }
                    };

                    // Pick R, G, B based on Bayer pattern (unchanged)
                    let local_x = sensor_x - block_x;
                    let local_y = sensor_y - block_y;

                    let (r, g, b) = match ui.bayer_pattern {
                        0 => {
                            // RGGB: R G / G B
                            let g = if local_x + local_y < 2 { tr } else { bl };
                            (tl, g, br)
                        }
                        1 => {
                            // GRBG: G R / B G
                            let g = if local_x + local_y < 2 { tl } else { br };
                            (tr, g, bl)
                        }
                        2 => {
                            // GBRG: G B / R G
                            let g = if local_x + local_y < 2 { tl } else { br };
                            (bl, g, tr)
                        }
                        3 => {
                            // BGGR: B G / G R
                            let g = if local_x + local_y < 2 { tr } else { bl };
                            (br, g, tl)
                        }
                        _ => panic!("Unknown bayer pattern!"),
                    };

                    // Apply display scaling based on mode
                    let scale = match current_mode {
                        RawMode::Average => {
                            ui.display_gain as f32 * (65536. / (65536. - ui.raw_black_level as f32))
                        }
                        RawMode::Difference | RawMode::Motion => ui.display_gain as f32,
                    };

                    // Linear scale, then apply the camera->Rec.2020 display matrix in
                    // LINEAR space (mixes channels), then sqrt-encode for the BT.2020
                    // surface. Average mode is colour (debayered) so colour-correct;
                    // diff/motion are monochrome magnitudes - leave them unmixed.
                    let (mut lr, mut lg, mut lb) =
                        (r as f32 * scale, g as f32 * scale, b as f32 * scale);

                    // Composite the in-place calibration overlay in linear space, before the
                    // display matrix. Overlay coords are in raw sensor frame (1x); map this
                    // pixel's sensor coord directly into it.
                    if let Some((min_x, min_y, ov_w, ov_h, overlay)) = cal_overlay {
                        let ox = sensor_x;
                        let oy = sensor_y;
                        if ox >= *min_x && ox < min_x + ov_w && oy >= *min_y && oy < min_y + ov_h {
                            let ov_idx = ((oy - min_y) * ov_w + (ox - min_x)) * 4;
                            if ov_idx + 3 < overlay.len() {
                                let alpha = overlay[ov_idx + 3];
                                if alpha > 0.0 {
                                    // Overlay 0..1 linear -> lumis linear units (white * scale).
                                    let k = 65535.0 * scale;
                                    let ia = 1.0 - alpha;
                                    lr = overlay[ov_idx] * k * alpha + lr * ia;
                                    lg = overlay[ov_idx + 1] * k * alpha + lg * ia;
                                    lb = overlay[ov_idx + 2] * k * alpha + lb * ia;
                                }
                            }
                        }
                    }

                    let (lr, lg, lb) = if current_mode == RawMode::Average {
                        apply_display_matrix(ui, lr, lg, lb)
                    } else {
                        (lr, lg, lb)
                    };
                    pixels[dst_idx] = (lr.max(0.).sqrt()) as u8;
                    pixels[dst_idx + 1] = (lg.max(0.).sqrt()) as u8;
                    pixels[dst_idx + 2] = (lb.max(0.).sqrt()) as u8;
                }
            }
        } // End of fit-to-screen mode
    } // End of image rendering block
}

fn debug_draw_margins(ui: &UserInterface, pixels: &mut [u8], buffer: &ANativeWindow_Buffer) {
    let stride = buffer.stride as usize;
    let height = buffer.height as usize;
    let width = buffer.width as usize;
    for y in 0..height {
        for x in 0..width {
            let in_x_margin =
                (x as f32) < ui.x_margin || (x as f32) > (ui.screen_run as f32 - ui.x_margin);
            let in_y_margin =
                (y as f32) < ui.y_margin || (y as f32) > (ui.screen_rise as f32 - ui.y_margin);
            if in_x_margin || in_y_margin {
                let idx = (y * stride + x) * 3;
                pixels[idx] = pixels[idx] + 16;
                pixels[idx + 1] = pixels[idx + 1] + 16;
                pixels[idx + 2] = pixels[idx + 2] + 16;
            }
        }
    }
}

fn draw_progress(ui: &mut UserInterface, pixels: &mut [u8], buffer: &ANativeWindow_Buffer) {
    // Get exposure start time from camera integrator via SharedMemory
    let start_secs = ui.header[EXPOSURE_START_SECS_IDX];
    let start_nanos = ui.header[EXPOSURE_START_NANOS_IDX] as u32;

    let current_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let exposure_start_unix = std::time::Duration::new(start_secs, start_nanos);
    let elapsed = (current_unix - exposure_start_unix).as_millis() as u64;

    let total_ms = ui.exposure_time_ms;
    let remaining = total_ms.saturating_sub(elapsed);

    let elapsed_str = ui.format_time(elapsed.min(total_ms));
    let remaining_str = ui.format_time(remaining);

    // Draw elapsed text (bottom-left) using stored position
    ui.text_renderer.draw_text_right(
        pixels,
        buffer.stride as u32,
        buffer.height as u32,
        &elapsed_str,
        ui.left_label_draw_x,
        ui.left_label_draw_y,
        ui.label_text_height,
        200,
        253,
        128,
        255,
        ui.device_rotation as u16,
    );

    // Draw remaining text (bottom-right) using stored position
    ui.text_renderer.draw_text_left(
        pixels,
        buffer.stride as u32,
        buffer.height as u32,
        &remaining_str,
        ui.right_label_draw_x,
        ui.right_label_draw_y,
        ui.label_text_height,
        200,
        253,
        128,
        255,
        ui.device_rotation as u16,
    );

    // Draw green progress bar on exposure time slider
    if ui.slider_thickness > 0.0 && total_ms > 0 {
        // Calculate progress based on timescale, not actual exposure duration
        let timescale_ms = ui.time_base.duration_ms();
        let progress = (elapsed as f64 / timescale_ms).sqrt().min(1.0) as f32;

        // Determine if slider is horizontal or vertical
        let is_vertical = (ui.slider_start_x - ui.slider_end_x).abs() < 1.0;

        if is_vertical {
            // Vertical slider - interpolate Y coordinates
            let progress_y = ui.slider_start_y + (ui.slider_end_y - ui.slider_start_y) * progress;
            let thickness = ui.slider_thickness as i32;

            let start_y = ui.slider_start_y as usize;
            let end_y = progress_y as usize;
            let center_x = ui.slider_start_x as i32;

            for y in start_y.min(end_y)..=start_y.max(end_y) {
                for dx in -thickness..=thickness {
                    let x = center_x + dx;
                    if x >= 0 && (x as usize) < buffer.stride as usize {
                        let idx = (y * buffer.stride as usize + x as usize) * 3;
                        if idx + 2 < pixels.len() {
                            pixels[idx] = 0; // R
                            pixels[idx + 1] = 255; // G
                            pixels[idx + 2] = 0; // B
                        }
                    }
                }
            }
        } else {
            // Horizontal slider - interpolate X coordinates
            let progress_x = ui.slider_start_x + (ui.slider_end_x - ui.slider_start_x) * progress;
            let thickness = ui.slider_thickness as i32;

            let start_x = ui.slider_start_x as usize;
            let end_x = progress_x as usize;
            let center_y = ui.slider_start_y as i32;

            for x in start_x.min(end_x)..=start_x.max(end_x) {
                for dy in -thickness..=thickness {
                    let y = center_y + dy;
                    if y >= 0 && (y as usize) < buffer.height as usize {
                        let idx = (y as usize * buffer.stride as usize + x) * 3;
                        if idx + 2 < pixels.len() {
                            pixels[idx] = 0; // R
                            pixels[idx + 1] = 255; // G
                            pixels[idx + 2] = 0; // B
                        }
                    }
                }
            }
        }
    }

    // Update cached text
    ui.last_elapsed_text = elapsed_str;
    ui.last_remaining_text = remaining_str;
}

fn draw_counters(ui: &mut UserInterface, pixels: &mut [u8], buffer: &ANativeWindow_Buffer) {
    // Check save flags
    let flags = ui.header[FLAGS_IDX];
    let fps = f64::from_bits(ui.header[FPS_IDX]);

    // Format counter strings
    let saved_text = format!("{}:S", ui.header[SAVED_COUNTER_IDX]);
    let frame_text = format!("{}:I", ui.header[FRAME_COUNTER_IDX].max(1));
    let fps_text = format!("{:.1}:F", fps);
    // 4th line: the selected save format (tap the counter block to cycle).
    let format_text = match ui.header[SAVE_FORMAT_IDX] {
        SAVE_FORMAT_JPEG => "JPEG",
        SAVE_FORMAT_TIFF => "TIFF",
        SAVE_FORMAT_DNG => "DNG",
        SAVE_FORMAT_JPEGXL => "JXL",
        _ => "JPEG",
    }
    .to_owned();

    // Calculate text positions (right-aligned, vertically stacked)
    let text_height = ui.screen_rise.min(ui.screen_run) as f32 * 0.032;

    let space = if ui.is_fat() {
        0.05
    } else {
        0.05 * ui.screen_aspect
    };

    // Convert to screen coordinates using user_to_screen
    let positions = [
        user_to_screen(ui, 1., space * 0.5), // Saved
        user_to_screen(ui, 1., space * 1.5), // Frame
        user_to_screen(ui, 1., space * 2.5), // FPS
        user_to_screen(ui, 1., space * 3.5), // Save format
    ];

    let saved_colour = if (flags & MANUAL_SAVE_BIT) != 0 {
        (32, 255, 0)
    } else {
        (127, 127, 127)
    };

    let frame_count_colour = if (flags & CONTINUOUS_SAVE_BIT) != 0 {
        (0, 128, 255)
    } else {
        (127, 127, 127)
    };

    // FPS colour - based on ratio to shutter speed
    let shutter_ns = f64::from_bits(ui.header[SHUTTER_NS_IDX]);
    let theoretical_max_fps = 1_000_000_000. / shutter_ns;
    let fps_ratio = fps / theoretical_max_fps;
    let fps_colour = (((1. - fps_ratio) * 256.) as u8, (fps_ratio * 256.) as u8, 0);

    // Save-format line: cyan-ish, brighter while a save is pending.
    let format_colour = (128, 200, 255);

    // Draw the counters
    let texts = [&saved_text, &frame_text, &fps_text, &format_text];
    let colours = [saved_colour, frame_count_colour, fps_colour, format_colour];

    for (i, (text, colour)) in texts.iter().zip(colours.iter()).enumerate() {
        ui.text_renderer.draw_text_right(
            pixels,
            buffer.stride as u32,
            buffer.height as u32,
            text,
            positions[i].0,
            positions[i].1,
            text_height,
            300,
            colour.0,
            colour.1,
            colour.2,
            ui.device_rotation as u16,
        );
    }
}

// Save counter areas for partial redraw
pub fn save_counter_areas(ui: &mut UserInterface, pixels: &[u8], stride: usize) {
    ui.counter_areas.clear();

    // Calculate positions same as draw_counters
    let (space, width) = if ui.is_fat() {
        (0.05, 0.2 * ui.screen_aspect)
    } else {
        (0.05 * ui.screen_aspect, 0.2)
    };
    let fudge = 0.003;

    // Counter positions in user coordinates
    let user_positions = [
        ((1., fudge), (1. - width, space - fudge)), // Saved
        ((1., space + fudge), (1. - width, space * 2. - fudge)), // Frame
        ((1., space * 2. + fudge), (1. - width, space * 3. - fudge)), // FPS
        ((1., space * 3. + fudge), (1. - width, space * 4. - fudge)), // Save format
    ];

    for (i, &((tl_x_user, tl_y_user), (br_x_user, br_y_user))) in user_positions.iter().enumerate()
    {
        // Transform corners to screen coordinates
        let (tl_x, tl_y) = user_to_screen(ui, tl_x_user, tl_y_user);
        let (br_x, br_y) = user_to_screen(ui, br_x_user, br_y_user);

        // Find actual screen bounds after rotation
        let x_start = tl_x.min(br_x) as usize;
        let x_end = tl_x.max(br_x) as usize;
        let y_start = tl_y.min(br_y) as usize;
        let y_end = tl_y.max(br_y) as usize;

        ui.counter_areas.push((x_start, y_start, x_end, y_end));

        // Save pixels
        let width = x_end - x_start;
        let height = y_end - y_start;
        let buffer_size = width * height * 3;
        ui.counter_buffers[i] = Vec::with_capacity(buffer_size);

        for y in y_start..y_end {
            for x in x_start..x_end {
                let idx = (y * stride + x) * 3;
                ui.counter_buffers[i].push(pixels[idx]);
                ui.counter_buffers[i].push(pixels[idx + 1]);
                ui.counter_buffers[i].push(pixels[idx + 2]);
            }
        }
    }
}

// Restore counter areas for partial redraw
pub fn restore_counter_areas(ui: &UserInterface, pixels: &mut [u8], stride: usize) {
    for (i, area) in ui.counter_areas.iter().enumerate() {
        if ui.counter_buffers[i].is_empty() {
            continue;
        }

        let (x_start, y_start, x_end, y_end) = *area;
        let mut buf_idx = 0;

        for y in y_start..y_end {
            for x in x_start..x_end {
                let dst_idx = (y * stride + x) * 3;
                pixels[dst_idx] = ui.counter_buffers[i][buf_idx];
                pixels[dst_idx + 1] = ui.counter_buffers[i][buf_idx + 1];
                // +32 blue is a debug tint to visualise the counter restore region; in
                // production restore the original pixel untinted. (Wrapping add, like the
                // rest of the codebase; the tint values never approach overflow anyway.)
                pixels[dst_idx + 2] = if crate::DEBUG {
                    ui.counter_buffers[i][buf_idx + 2] + 32
                } else {
                    ui.counter_buffers[i][buf_idx + 2]
                };
                buf_idx += 3;
            }
        }
    }
}
