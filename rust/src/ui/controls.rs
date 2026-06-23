use crate::{
    ui::{
        buttons::{composite_calibration_scan, composite_chameleon_button},
        sliders::get_slider_labels,
        touch::slider_to_horizontal,
        ui::ui_constants,
    },
    UserInterface,
};
use ndk_sys::ANativeWindow_Buffer;

// Convert from user-relative coordinates inside margins to native screen pixel coordinates
// horizontal: 0.0 = left edge of content area, 1.0 = right edge of content area
// vertical: 0.0 = top edge of content area, 1.0 = bottom edge of content area
pub fn user_to_screen(ui: &UserInterface, horizontal: f32, vertical: f32) -> (f32, f32) {
    // Apply transformations
    let h = if ui.is_fat() ^ (ui.device_rotation > 127) {
        1. - horizontal
    } else {
        horizontal
    };
    let v = if ui.device_rotation > 127 {
        1. - vertical
    } else {
        vertical
    };

    // Apply axis swap and convert to screen space
    let (x_unscaled, y_unscaled) = if ui.is_fat() { (v, h) } else { (h, v) };

    let x = x_unscaled * (ui.screen_run as f32 - ui.x_margin * 2.) + ui.x_margin;
    let y = y_unscaled * (ui.screen_rise as f32 - ui.y_margin * 2.) + ui.y_margin;

    (x, y)
}

// [margin]
// [margin][empty space][margin]
// [margin][time button][empty space][histogram button][margin]
// [margin][mode button][empty space][exit button][margin]
// [margin][exposure label][gap][arrow][slider][arrow][gap][exposure label][margin]
// [margin][iso label][gap][arrow][slider][arrow][gap][iso label][margin]
// [margin][shutter label][gap][arrow][slider][arrow][gap][shutter label][margin]
// [margin][focus label][gap][arrow][slider][arrow][gap][focus label][margin]
// [margin][gain label][gap][arrow][slider][arrow][gap][gain label][margin]
// [margin]

pub fn draw_controls(
    ui: &mut UserInterface,
    pixels: &mut [u8],
    screen_buffer: &ANativeWindow_Buffer,
) {
    // Always draw calibration button regardless of controls_visible
    let (cal_x, cal_y) = user_to_screen(ui, 0., 0.);
    let is_pressed = ui.active_button == Some(4); // Calibration is button index 4
    let is_calibrating = ui.calibrating.load(std::sync::atomic::Ordering::Relaxed);

    // After a successful calibration we show the cropped target scan in place of the
    // button as a "calibrated" indicator. While actively calibrating, keep the blinking
    // button so the in-progress feedback isn't lost.
    let overlay = ui.calibration_overlay.load();
    if let (false, Some((ow, oh, img))) = (is_calibrating, overlay.as_ref()) {
        composite_calibration_scan(
            pixels,
            cal_x as usize,
            cal_y as usize,
            ui.device_rotation as u16,
            ui.calibration_button_buffer.size,
            *ow,
            *oh,
            img,
            is_pressed,
            screen_buffer.stride as usize,
        );
    } else {
        composite_chameleon_button(
            pixels,
            cal_x as usize,
            cal_y as usize,
            ui.device_rotation as u16,
            &ui.calibration_button_buffer,
            is_pressed,
            is_calibrating,
            screen_buffer.stride as usize,
        );
    }

    // Only draw other controls if controls_visible is true
    if ui.controls_visible {
        let diagonal =
            ((ui.screen_run * ui.screen_run + ui.screen_rise * ui.screen_rise) as f32).sqrt();
        let circle_radius = (diagonal * ui_constants::SLIDER_CIRCLE_DIAGONAL) as i32;

        let slider_values = [
            ui.gain_slider,
            ui.focus_slider,
            ui.shutter_slider,
            ui.iso_slider,
            ui.exposure_time_slider,
        ];

        let (controls_ratio, slider_start, arrow_offset, mut text_height, label_offset) =
            if ui.is_fat() {
                (
                    ui_constants::CONTROLS_HEIGHT_FAT,
                    ui_constants::LABEL_WIDTH * ui.screen_aspect,
                    ui_constants::ARROW_OFFSET * ui.screen_aspect,
                    ui_constants::CONTROLS_HEIGHT_FAT * ui.screen_rise.min(ui.screen_run) as f32,
                    ui_constants::LABEL_OFFSET * ui.screen_aspect,
                )
            } else {
                (
                    ui_constants::CONTROLS_HEIGHT_SKINNY,
                    ui_constants::LABEL_WIDTH,
                    ui_constants::ARROW_OFFSET,
                    ui_constants::CONTROLS_HEIGHT_SKINNY * ui.screen_rise.max(ui.screen_run) as f32,
                    ui_constants::LABEL_OFFSET,
                )
            };
        text_height *= ui_constants::LABEL_HEIGHT / ui_constants::TOTAL_TRACKS as f32;
        let track_height = controls_ratio / ui_constants::TOTAL_TRACKS as f32;

        let slider_end = 1. - slider_start;

        // Draw all tracks (0-6)
        for track in 0..ui_constants::TOTAL_TRACKS {
            // Calculate track position in logical space (0.0-1.0)
            let track_bottom = 1. - (track as f32) * track_height;
            let track_center = 1. - (track as f32 + 0.5) * track_height;
            // let track_top = 1. - (track as f32 + 1.) * track_height;

            if track < 5 {
                let slider_value = slider_values[track];
                let slider_horizontal = slider_to_horizontal(ui, slider_value as f32);

                let (start_x, start_y) = user_to_screen(ui, slider_start, track_center);
                let (pos_x, pos_y) = user_to_screen(ui, slider_horizontal, track_center);
                let (end_x, end_y) = user_to_screen(ui, slider_end, track_center);

                for y in start_y.min(pos_y) as usize..=start_y.max(pos_y) as usize {
                    for x in start_x.min(pos_x) as usize..=start_x.max(pos_x) as usize {
                        let idx = (y * screen_buffer.stride as usize + x) * 3;
                        pixels[idx] = 255;
                        pixels[idx + 1] = 255;
                        pixels[idx + 2] = 255;
                    }
                }
                for y in end_y.min(pos_y) as usize..=end_y.max(pos_y) as usize {
                    for x in end_x.min(pos_x) as usize..=end_x.max(pos_x) as usize {
                        let idx = (y * screen_buffer.stride as usize + x) * 3;
                        pixels[idx] = 0;
                        pixels[idx + 1] = 0;
                        pixels[idx + 2] = 0;
                    }
                }

                // Draw slider circle
                draw_filled_circle(
                    pixels,
                    pos_x as i32,
                    pos_y as i32,
                    circle_radius,
                    screen_buffer.stride,
                );

                // Draw left arrow
                let (left_arrow_x, left_arrow_y) = user_to_screen(ui, arrow_offset, track_center);
                let needs_flipped = if ui.is_fat() {
                    // Portrait orientations (90°/270°) - flip logic is inverted
                    ui.device_rotation <= 127
                } else {
                    // Landscape orientations (0°/180°) - normal flip logic
                    ui.device_rotation > 127
                };
                let left_arrow_pressed = ui.pressed_arrow == Some((track, false)); // false = decrease (left arrow)
                crate::ui::arrows::composite_arrow(
                    pixels,
                    left_arrow_x as usize,
                    left_arrow_y as usize,
                    &ui.arrow_buffers,
                    ui.is_fat(),
                    left_arrow_pressed,
                    needs_flipped,
                    screen_buffer.stride as usize,
                );

                // Draw right arrow
                let (right_arrow_x, right_arrow_y) =
                    user_to_screen(ui, 1.0 - arrow_offset, track_center);
                let right_arrow_pressed = ui.pressed_arrow == Some((track, true)); // true = increase (right arrow)
                crate::ui::arrows::composite_arrow(
                    pixels,
                    right_arrow_x as usize,
                    right_arrow_y as usize,
                    &ui.arrow_buffers,
                    ui.is_fat(),
                    right_arrow_pressed,
                    !needs_flipped, // Right arrow has opposite flip from left
                    screen_buffer.stride as usize,
                );

                // Draw labels
                let labels = get_slider_labels(ui, track);
                if track == 4 {
                    // Cache slider coordinates for progress bar
                    ui.slider_start_x = start_x;
                    ui.slider_start_y = start_y;
                    ui.slider_end_x = end_x;
                    ui.slider_end_y = end_y;
                    ui.slider_thickness = circle_radius as f32 / 2.0;

                    save_label_areas(
                        ui,
                        pixels,
                        screen_buffer.stride as usize,
                        track_center,
                        track_bottom,
                        label_offset,
                        track_height,
                        text_height,
                    );
                } else {
                    let (label_x, label_y) = user_to_screen(
                        ui,
                        label_offset,
                        track_center + ui_constants::LABEL_VGAP * track_height,
                    );
                    ui.text_renderer.draw_text_right(
                        pixels,
                        screen_buffer.stride as u32,
                        screen_buffer.height as u32,
                        &labels.2 .0,
                        label_x,
                        label_y,
                        text_height,
                        200,
                        labels.0 .1[0],
                        labels.0 .1[1],
                        labels.0 .1[2],
                        ui.device_rotation as u16,
                    );

                    let (label_x, label_y) = user_to_screen(
                        ui,
                        1. - label_offset,
                        track_center + ui_constants::LABEL_VGAP * track_height,
                    );
                    ui.text_renderer.draw_text_left(
                        pixels,
                        screen_buffer.stride as u32,
                        screen_buffer.height as u32,
                        &labels.3 .0,
                        label_x,
                        label_y,
                        text_height,
                        200,
                        labels.2 .1[0],
                        labels.2 .1[1],
                        labels.2 .1[2],
                        ui.device_rotation as u16,
                    );
                }
                let (label_x, label_y) = user_to_screen(
                    ui,
                    label_offset,
                    track_center - ui_constants::LABEL_VGAP * track_height,
                );
                ui.text_renderer.draw_text_right(
                    pixels,
                    screen_buffer.stride as u32,
                    screen_buffer.height as u32,
                    &labels.0 .0,
                    label_x,
                    label_y,
                    text_height,
                    400,
                    labels.1 .1[0],
                    labels.1 .1[1],
                    labels.1 .1[2],
                    ui.device_rotation as u16,
                );

                let (label_x, label_y) = user_to_screen(
                    ui,
                    1. - label_offset,
                    track_center - ui_constants::LABEL_VGAP * track_height,
                );
                ui.text_renderer.draw_text_left(
                    pixels,
                    screen_buffer.stride as u32,
                    screen_buffer.height as u32,
                    &labels.1 .0,
                    label_x,
                    label_y,
                    text_height,
                    400,
                    labels.3 .1[0],
                    labels.3 .1[1],
                    labels.3 .1[2],
                    ui.device_rotation as u16,
                );
            } else {
                // Draw buttons (tracks 5-6)
                let button_index_base = (track - 5) * 2;

                // Left button
                let left_button_index = button_index_base;
                if left_button_index < 4 {
                    let (x, y) = if ui.is_fat() {
                        user_to_screen(ui, ui_constants::FAT_BUTTON_WIDTH / 2., track_center)
                    } else {
                        user_to_screen(ui, ui_constants::SKINNY_BUTTON_WIDTH / 2., track_center)
                    };
                    let is_pressed = ui.active_button == Some(left_button_index);

                    crate::ui::buttons::composite_button(
                        pixels,
                        x as usize,
                        y as usize,
                        &ui.button_buffers,
                        left_button_index,
                        ui.is_fat(),
                        is_pressed,
                        screen_buffer.stride as usize,
                    );

                    // Draw button text
                    let (text, color) = ui.get_button_info(left_button_index, is_pressed);
                    if !text.is_empty() {
                        let (width, height, mut text_height) = if ui.is_fat() {
                            (ui.button_buffers.fat_run, ui.button_buffers.fat_rise, 1.)
                        } else {
                            (
                                ui.button_buffers.skinny_run,
                                ui.button_buffers.skinny_rise,
                                0.8,
                            )
                        };
                        text_height *= (height.min(width) as f32 * 0.4) as f32;

                        ui.text_renderer.draw_text_center(
                            pixels,
                            screen_buffer.stride as u32,
                            ui.screen_rise as u32,
                            &text,
                            x,
                            y,
                            text_height,
                            200,
                            color[0],
                            color[1],
                            color[2],
                            ui.device_rotation as u16,
                        );
                    }
                }

                // Right button
                let right_button_index = button_index_base + 1;
                if right_button_index < 4 {
                    let (x, y) = if ui.is_fat() {
                        user_to_screen(ui, 1. - ui_constants::FAT_BUTTON_WIDTH / 2., track_center)
                    } else {
                        user_to_screen(
                            ui,
                            1. - ui_constants::SKINNY_BUTTON_WIDTH / 2.,
                            track_center,
                        )
                    };
                    let is_pressed = ui.active_button == Some(right_button_index);

                    crate::ui::buttons::composite_button(
                        pixels,
                        x as usize,
                        y as usize,
                        &ui.button_buffers,
                        right_button_index,
                        ui.is_fat(),
                        is_pressed,
                        screen_buffer.stride as usize,
                    );

                    // Draw button text
                    let (text, color) = ui.get_button_info(right_button_index, is_pressed);
                    if !text.is_empty() {
                        let (width, height, mut text_height) = if ui.is_fat() {
                            (ui.button_buffers.fat_run, ui.button_buffers.fat_rise, 1.)
                        } else {
                            (
                                ui.button_buffers.skinny_run,
                                ui.button_buffers.skinny_rise,
                                0.8,
                            )
                        };
                        text_height *= (height.min(width) as f32 * 0.4) as f32;

                        ui.text_renderer.draw_text_center(
                            pixels,
                            screen_buffer.stride as u32,
                            ui.screen_rise as u32,
                            &text,
                            x,
                            y,
                            text_height,
                            200,
                            color[0],
                            color[1],
                            color[2],
                            ui.device_rotation as u16,
                        );
                    }
                }
            }
        }
    } // End of controls_visible block
}

pub fn save_label_areas(
    ui: &mut UserInterface,
    pixels: &[u8],
    stride: usize,
    track_center: f32,
    track_bottom: f32,
    label_offset: f32,
    track_height: f32,
    text_height: f32,
) {
    // Store label drawing positions for partial redraw
    let (left_x, left_y) = user_to_screen(
        ui,
        label_offset,
        track_center + ui_constants::LABEL_VGAP * track_height,
    );
    ui.left_label_draw_x = left_x;
    ui.left_label_draw_y = left_y;

    let (right_x, right_y) = user_to_screen(
        ui,
        1. - label_offset,
        track_center + ui_constants::LABEL_VGAP * track_height,
    );
    ui.right_label_draw_x = right_x;
    ui.right_label_draw_y = right_y;
    ui.label_text_height = text_height;

    // Save pixel area under labels
    let (tl_x, tl_y) = user_to_screen(ui, -1., track_center);
    let (br_x, br_y) = user_to_screen(ui, label_offset, track_bottom);

    ui.left_label_x = tl_x.min(br_x) as usize;
    ui.left_label_end_x = (tl_x.max(br_x) as usize).min(ui.screen_run);
    ui.left_label_y = tl_y.min(br_y) as usize;
    ui.left_label_end_y = (tl_y.max(br_y) as usize).min(ui.screen_rise);

    let width = ui.left_label_end_x - ui.left_label_x + 1;
    let height = ui.left_label_end_y - ui.left_label_y + 1;

    let buffer_size = width * height * 3;
    ui.left_label_buffer = Vec::with_capacity(buffer_size);

    for y in ui.left_label_y..ui.left_label_end_y {
        for x in ui.left_label_x..ui.left_label_end_x {
            let src_idx = (y * stride as usize + x) * 3;
            ui.left_label_buffer.push(pixels[src_idx]);
            ui.left_label_buffer.push(pixels[src_idx + 1]);
            ui.left_label_buffer.push(pixels[src_idx + 2]);
        }
    }

    let (tl_x, tl_y) = user_to_screen(ui, 1. - label_offset, track_center);
    let (br_x, br_y) = user_to_screen(ui, 2., track_bottom);

    ui.right_label_x = tl_x.min(br_x) as usize;
    ui.right_label_end_x = (tl_x.max(br_x) as usize).min(ui.screen_run);
    ui.right_label_y = tl_y.min(br_y) as usize;
    ui.right_label_end_y = (tl_y.max(br_y) as usize).min(ui.screen_rise);

    let width = ui.right_label_end_x - ui.right_label_x + 1;
    let height = ui.right_label_end_y - ui.right_label_y + 1;

    let buffer_size = width * height * 3;
    ui.right_label_buffer = Vec::with_capacity(buffer_size);

    for y in ui.right_label_y..ui.right_label_end_y {
        for x in ui.right_label_x..ui.right_label_end_x {
            let src_idx = (y * stride as usize + x) * 3;
            ui.right_label_buffer.push(pixels[src_idx]);
            ui.right_label_buffer.push(pixels[src_idx + 1]);
            ui.right_label_buffer.push(pixels[src_idx + 2]);
        }
    }
}

pub fn restore_label_areas(ui: &UserInterface, pixels: &mut [u8], stride: usize) {
    // Restore left label area
    let mut buf_idx = 0;
    for y in ui.left_label_y..ui.left_label_end_y {
        for x in ui.left_label_x..ui.left_label_end_x {
            let dst_idx = (y * stride + x) * 3;
            pixels[dst_idx] = ui.left_label_buffer[buf_idx];
            if crate::DEBUG {
                pixels[dst_idx + 1] = ui.left_label_buffer[buf_idx + 1] + 24;
            } else {
                pixels[dst_idx + 1] = ui.left_label_buffer[buf_idx + 1];
            }
            pixels[dst_idx + 2] = ui.left_label_buffer[buf_idx + 2];
            buf_idx += 3;
        }
    }

    // Restore right label area
    buf_idx = 0;
    for y in ui.right_label_y..ui.right_label_end_y {
        for x in ui.right_label_x..ui.right_label_end_x {
            let dst_idx = (y * stride + x) * 3;
            pixels[dst_idx] = ui.right_label_buffer[buf_idx];
            pixels[dst_idx + 1] = ui.right_label_buffer[buf_idx + 1];
            if crate::DEBUG {
                pixels[dst_idx + 2] = ui.right_label_buffer[buf_idx + 2] + 24;
            } else {
                pixels[dst_idx + 2] = ui.right_label_buffer[buf_idx + 2];
            }
            buf_idx += 3;
        }
    }
}

fn draw_filled_circle(
    pixels: &mut [u8],
    center_x: i32,
    center_y: i32,
    radius: i32,
    buffer_stride: i32,
) {
    let r = radius as f32 * 0.7;
    let g = radius as f32 * 0.8;
    let b = radius as f32 * 0.9;

    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let px = center_x + dx;
            let py = center_y + dy;
            let dst_idx = (py * buffer_stride + px) * 3;

            // Red channel - tightest, most intense
            let x = dx as f32 / r;
            let y = dy as f32 / r;
            let mut v = x * x + y * y;
            v = v * v;
            let fill_r = (0.5 * (2. - ((1. + 2f32.sqrt()) * v - 1.).powi(2))).max(0.);
            let fill_r = fill_r * 1.25;
            v = v * v;
            pixels[dst_idx as usize] =
                (pixels[dst_idx as usize] as f32 * v.min(1.) + fill_r * 256.) as u8;

            // Green channel - medium
            let x = dx as f32 / g;
            let y = dy as f32 / g;
            let mut v = x * x + y * y;
            v = v * v;
            let fill_g = (0.5 * (2. - ((1. + 2f32.sqrt()) * v - 1.).powi(2))).max(0.);
            v = v * v;
            pixels[dst_idx as usize + 1] =
                (pixels[dst_idx as usize + 1] as f32 * v.min(1.) + fill_g * 256.) as u8;

            // Blue channel - widest, least intense
            let x = dx as f32 / b;
            let y = dy as f32 / b;
            let mut v = x * x + y * y;
            v = v * v;
            let fill_b = (0.5 * (2. - ((1. + 2f32.sqrt()) * v - 1.).powi(2))).max(0.);
            let fill_b = fill_b * 0.625;
            v = v * v;
            pixels[dst_idx as usize + 2] =
                (pixels[dst_idx as usize + 2] as f32 * v.min(1.) + fill_b * 256.) as u8;
        }
    }
}
