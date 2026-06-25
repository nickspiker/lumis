use crate::{
    shared_memory::*,
    ui::{
        controls::user_to_screen,
        ui::ui_constants::{self, CALIBRATION_BUTTON_SIZE},
    },
    UserInterface,
};
use log::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TouchAction {
    Down,
    Hold,
    Up,
}

// Handle touch events - returns true if redraw is needed, false otherwise
pub fn handle_touch(
    ui: &mut UserInterface,
    action: TouchAction,
    x: f32,
    y: f32,
    _pressure: f32,
    _pointer_id: i32,
) -> bool {
    if crate::DEBUG {
        trace!("Touch event: action={:?}, pos=({:.1}, {:.1})", action, x, y);
    }

    // After finalize the dark frame is shown; a tap exits the app (same mechanism as the UI exit path:
    // clear the result bit, stale the heartbeat so the camera process auto-nukes, then exit). Relaunching
    // boots back to the camera-select menu - the app has no in-process camera->menu teardown, so a clean
    // exit is the simplest correct "done" action here.
    if (ui.header[crate::shared_memory::FLAGS_IDX] & crate::shared_memory::CAL_SHOW_RESULT_BIT) != 0 {
        if let TouchAction::Down = action {
            ui.header[crate::shared_memory::FLAGS_IDX] &=
                !crate::shared_memory::CAL_SHOW_RESULT_BIT;
            ui.header[crate::shared_memory::FLAGS_IDX] &= !crate::shared_memory::CONTINUOUS_SAVE_BIT;
            ui.header[crate::shared_memory::HEARTBEAT_SECS_IDX] -= 256;
            unsafe {
                libc::exit(0);
            }
        }
        return true;
    }

    // Dark-frame calibration screen: the only interactive element is FINALIZE. A tap inside that
    // button requests finalize+stop (the integrator writes the cal to disk and clears CALIBRATING_BIT);
    // everything else on this screen is inert. Handled before all normal camera-touch logic.
    if (ui.header[crate::shared_memory::FLAGS_IDX] & crate::shared_memory::CALIBRATING_BIT) != 0 {
        if let TouchAction::Down = action {
            let (bx0, by0, bx1, by1) = crate::ui::screen::calibration_finalize_rect(
                ui.screen_run as u32,
                ui.screen_rise as u32,
            );
            if x >= bx0 as f32 && x < bx1 as f32 && y >= by0 as f32 && y < by1 as f32 {
                ui.header[crate::shared_memory::FLAGS_IDX] |=
                    crate::shared_memory::CAL_FINALIZE_BIT;
            }
        }
        return true; // keep the stats screen repainting; consume all touches
    }

    match action {
        TouchAction::Down => {
            if crate::DEBUG {
                debug!("Touch start at ({:.1}, {:.1})", x, y);
            }

            // Tapping the top-right counter block cycles the save format. This lives in the margin (handled before the dead-zone check below). counter_areas is kept up to date by save_counter_areas during rendering.
            if point_in_counter_block(ui, x, y) {
                use crate::shared_memory::*;
                // Skip JXL in the cycle on devices whose MediaStore rejects image/jxl (flag = 2).
                let jxl_off = ui.header[JXL_SUPPORTED_IDX] == 2;
                let mut next =
                    (ui.header[SAVE_FORMAT_IDX] + 1) % SAVE_FORMAT_COUNT;
                if jxl_off && next == SAVE_FORMAT_JPEGXL {
                    next = (next + 1) % SAVE_FORMAT_COUNT;
                }
                ui.header[SAVE_FORMAT_IDX] = next;
                ui.touch_is_dead = true; // consume; don't also drive controls
                return true; // request redraw so the indicator updates
            }

            // Start touch
            ui.touch_start_x = x;
            ui.touch_start_y = y;
            ui.is_dragging = false;
            ui.touch_is_dead = false;

            // Check if we're in the margin dead zone
            if x < ui.x_margin
                || x > ui.screen_run as f32 - ui.x_margin
                || y < ui.y_margin
                || y > ui.screen_rise as f32 - ui.y_margin
            {
                if crate::DEBUG {
                    debug!(
                        "Touch in margin dead zone: x_margin={:.1}, y_margin={:.1}, screen={}x{}",
                        ui.x_margin, ui.y_margin, ui.screen_run, ui.screen_rise
                    );
                }
                // In margin - mark this touch as dead
                ui.touch_is_dead = true;
                return false;
            }

            // Check if touching a track element (only if controls visible)
            if ui.controls_visible {
                if crate::DEBUG {
                    debug!("Checking touch targets with controls visible");
                }

                if let Some(touch) = get_element_from_touch(ui, x, y) {
                    match touch {
                        TrackElement::Arrow { track, increase } => {
                            if crate::DEBUG {
                                debug!("Arrow pressed: track={}, increase={}", track, increase);
                            }
                            ui.pressed_arrow = Some((track, increase));

                            // Handle arrow logic - account for rotation 90° having inverted values
                            let should_invert = ui.device_rotation == 90 && ui.is_fat();
                            let delta = if should_invert != increase {
                                1. / 256.
                            } else {
                                -1. / 256.
                            };

                            if crate::DEBUG {
                                debug!(
                                    "Arrow delta: {:.6} (invert: {}, rotation: {}°)",
                                    delta, should_invert, ui.device_rotation
                                );
                            }

                            match track {
                                0 => {
                                    let old_gain = ui.gain_slider;
                                    ui.gain_slider = (ui.gain_slider + delta).clamp(0., 1.);
                                    ui.display_gain = 2f64.powf(ui.gain_slider * 12.);
                                    if crate::DEBUG {
                                        debug!(
                                            "Gain adjusted: {:.3} -> {:.3} (display_gain: {:.2})",
                                            old_gain, ui.gain_slider, ui.display_gain
                                        );
                                    }
                                }
                                1 => {
                                    let old_focus = ui.focus_slider;
                                    ui.focus_slider = (ui.focus_slider + delta).clamp(0., 1.);
                                    if crate::DEBUG {
                                        debug!(
                                            "Focus adjusted: {:.3} -> {:.3}",
                                            old_focus, ui.focus_slider
                                        );
                                    }
                                }
                                2 => {
                                    let old_shutter = ui.shutter_slider;
                                    ui.shutter_slider = (ui.shutter_slider + delta).clamp(0., 1.);
                                    if crate::DEBUG {
                                        debug!(
                                            "Shutter adjusted: {:.3} -> {:.3}",
                                            old_shutter, ui.shutter_slider
                                        );
                                    }
                                }
                                3 => {
                                    let old_iso = ui.iso_slider;
                                    ui.iso_slider = (ui.iso_slider + delta).clamp(0., 1.);
                                    if crate::DEBUG {
                                        debug!(
                                            "ISO adjusted: {:.3} -> {:.3}",
                                            old_iso, ui.iso_slider
                                        );
                                    }
                                }
                                4 => {
                                    let old_exposure = ui.exposure_time_slider;
                                    ui.exposure_time_slider =
                                        (ui.exposure_time_slider + delta).clamp(0., 1.);
                                    // Update exposure duration
                                    ui.exposure_time_ms = (ui.exposure_time_slider as f64
                                        * ui.exposure_time_slider as f64
                                        * ui.time_base.duration_ms())
                                        as u64;
                                    if crate::DEBUG {
                                        debug!(
                                            "Exposure time adjusted: {:.3} -> {:.3} (ms: {})",
                                            old_exposure,
                                            ui.exposure_time_slider,
                                            ui.exposure_time_ms
                                        );
                                    }
                                }
                                _ => {
                                    if crate::DEBUG {
                                        warn!("Unknown track index for arrow: {}", track);
                                    }
                                }
                            }

                            // Write to shared memory once after adjustment
                            update_shared_memory_for_track(ui, track);
                            return true;
                        }
                        TrackElement::Slider { track, value } => {
                            if crate::DEBUG {
                                debug!("Slider touched: track={}, value={:.3}", track, value);
                            }

                            // Set this as the active slider and update initial value
                            ui.active_track = Some(track);
                            match track {
                                0 => {
                                    ui.gain_slider = value as f64;
                                    ui.display_gain = 2f64.powf(value as f64 * 12.);
                                    if crate::DEBUG {
                                        debug!(
                                            "Gain slider set: {:.3} (display_gain: {:.2})",
                                            value, ui.display_gain
                                        );
                                    }
                                }
                                1 => {
                                    ui.focus_slider = value as f64;
                                    if crate::DEBUG {
                                        debug!("Focus slider set: {:.3}", value);
                                    }
                                }
                                2 => {
                                    ui.shutter_slider = value as f64;
                                    if crate::DEBUG {
                                        debug!("Shutter slider set: {:.3}", value);
                                    }
                                }
                                3 => {
                                    ui.iso_slider = value as f64;
                                    if crate::DEBUG {
                                        debug!("ISO slider set: {:.3}", value);
                                    }
                                }
                                4 => {
                                    ui.exposure_time_slider = value as f64;
                                    // Update exposure duration
                                    ui.exposure_time_ms = (ui.exposure_time_slider as f64
                                        * ui.exposure_time_slider as f64
                                        * ui.time_base.duration_ms())
                                        as u64;
                                    if crate::DEBUG {
                                        debug!("Exposure time slider set: {:.3}", value);
                                    }
                                }
                                _ => {
                                    if crate::DEBUG {
                                        warn!("Unknown track index for slider: {}", track);
                                    }
                                }
                            }

                            // Write to shared memory once after adjustment
                            update_shared_memory_for_track(ui, track);
                            return true;
                        }
                        TrackElement::Button { index } => {
                            if crate::DEBUG {
                                debug!("Button pressed: index={}", index);
                            }
                            ui.active_button = Some(index);
                            return true; // Just visual feedback, action on release
                        }
                    }
                } else {
                    if crate::DEBUG {
                        trace!("Touch not on any control element");
                    }
                }
            } else {
                if crate::DEBUG {
                    trace!("Controls not visible, skipping touch target check");
                }
            }

            // If we're in 1:1 mode and NOT on a slider, prepare for potential pan
            if ui.view_1to1 {
                if crate::DEBUG {
                    debug!("Touch start in 1:1 mode - pan ready");
                }
                // Will be handled in ACTION_UP if not a drag
                return false;
            }

            false
        }
        TouchAction::Hold => {
            if crate::DEBUG {
                trace!("Touch move to ({:.1}, {:.1})", x, y);
            }

            // Continue touch

            // If touch started in margin or was a button drag off cancel, ignore everything
            if ui.touch_is_dead {
                if crate::DEBUG {
                    trace!("Touch is dead, ignoring move");
                }
                return false;
            }

            // If we have an active slider, continue adjusting it
            if let Some(track_index) = ui.active_track {
                if crate::DEBUG {
                    trace!("Continuing slider adjustment for track {}", track_index);
                }

                // Normalize coordinates to 0-1 range (accounting for margins)
                let norm_x = (x - ui.x_margin) / (ui.screen_run as f32 - ui.x_margin * 2.);
                let norm_y = (y - ui.y_margin) / (ui.screen_rise as f32 - ui.y_margin * 2.);

                let (horizontal, _) = match (ui.is_fat(), ui.device_rotation > 127) {
                    (true, true) => (norm_y, norm_x),
                    (true, false) => (1. - norm_y, 1. - norm_x),
                    (false, true) => (1. - norm_x, norm_y),
                    (false, false) => (norm_x, 1. - norm_y),
                };

                // Convert to slider value and clamp
                let slider_value = horizontal_to_slider(ui, horizontal).clamp(0.0, 1.0) as f64;
                if crate::DEBUG {
                    trace!(
                        "Slider value calculated: {:.3} (horizontal: {:.3})",
                        slider_value,
                        horizontal
                    );
                }

                match track_index {
                    0 => {
                        ui.gain_slider = slider_value;
                        ui.display_gain = 2f64.powf(slider_value * 12.);
                    }
                    1 => {
                        ui.focus_slider = slider_value;
                    }
                    2 => {
                        ui.shutter_slider = slider_value;
                    }
                    3 => {
                        ui.iso_slider = slider_value;
                    }
                    4 => {
                        ui.exposure_time_slider = slider_value;
                        // Update exposure duration
                        ui.exposure_time_ms = (ui.exposure_time_slider as f64
                            * ui.exposure_time_slider as f64
                            * ui.time_base.duration_ms())
                            as u64;
                    }
                    _ => {
                        if crate::DEBUG {
                            warn!("Unknown active track: {}", track_index);
                        }
                    }
                }

                // Write to shared memory once after adjustment
                update_shared_memory_for_track(ui, track_index);
                return true;
            }

            // Check if we're dragging off a button
            if let Some(button_index) = ui.active_button {
                // First check if we dragged into the margin - instant cancel
                if x < ui.x_margin
                    || x > ui.screen_run as f32 - ui.x_margin
                    || y < ui.y_margin
                    || y > ui.screen_rise as f32 - ui.y_margin
                {
                    if crate::DEBUG {
                        debug!(
                            "Button drag into margin - canceling button {}",
                            button_index
                        );
                    }
                    ui.active_button = None;
                    ui.touch_is_dead = true;
                    return true;
                }

                // Check if we're still over the same button
                let mut still_on_button = false;

                // Check if we're still over the same button using consistent coordinate system
                if let Some(TrackElement::Button { index }) = get_element_from_touch(ui, x, y) {
                    still_on_button = index == button_index;
                }

                if !still_on_button {
                    if crate::DEBUG {
                        debug!("Button drag off button {} (not into margin)", button_index);
                    }
                    // Dragged off button entirely (but not into margin, we checked that above)
                    ui.active_button = None;
                    ui.touch_is_dead = true; // Prevent pan
                    return true;
                }
            }

            // Otherwise check for drag/pan
            let dx = x - ui.touch_start_x;
            let dy = y - ui.touch_start_y;
            let diagonal =
                ((ui.screen_run * ui.screen_run + ui.screen_rise * ui.screen_rise) as f32).sqrt();
            let drag_threshold = diagonal * ui_constants::DRAG_THRESHOLD;
            let drag_distance = (dx * dx + dy * dy).sqrt();

            if crate::DEBUG {
                trace!(
                    "Drag check: distance={:.1}, threshold={:.1}",
                    drag_distance,
                    drag_threshold
                );
            }

            // Check if we've moved enough to be a drag (but not in histogram mode)
            if !ui.is_dragging
                && !ui.histogram_visible
                && ui.active_button.is_none()
                && ui.pressed_arrow.is_none()
                && drag_distance > drag_threshold
            {
                if crate::DEBUG {
                    debug!("Drag threshold exceeded - starting drag mode");
                }
                ui.is_dragging = true;

                if !ui.view_1to1 {
                    if crate::DEBUG {
                        debug!("Entering 1:1 mode from drag");
                    }

                    // Enter 1:1 mode keeping content under finger in place
                    ui.view_1to1 = true;

                    let needs_rotated = ui.sensor_orientation == 90 || ui.sensor_orientation == 270;
                    let (effective_width, effective_height) = if needs_rotated {
                        (ui.sensor_y_size as f32, ui.sensor_x_size as f32)
                    } else {
                        (ui.sensor_x_size as f32, ui.sensor_y_size as f32)
                    };

                    if crate::DEBUG {
                        debug!(
                            "Sensor dimensions: {}x{}, effective: {:.0}x{:.0}, rotation: {}°",
                            ui.sensor_x_size,
                            ui.sensor_y_size,
                            effective_width,
                            effective_height,
                            ui.sensor_orientation
                        );
                    }

                    let scale_x = ui.screen_run as f32 / effective_width;
                    let scale_y = ui.screen_rise as f32 / effective_height;
                    let scale = scale_x.min(scale_y);

                    let scaled_width = effective_width * scale;
                    let scaled_height = effective_height * scale;
                    let display_offset_x = (ui.screen_run as f32 - scaled_width) / 2.0;
                    let display_offset_y = (ui.screen_rise as f32 - scaled_height) / 2.0;

                    if crate::DEBUG {
                        debug!("Scale calculation: scale={:.3}, scaled_size={:.0}x{:.0}, offset=({:.1}, {:.1})", 
                               scale, scaled_width, scaled_height, display_offset_x, display_offset_y);
                    }

                    // Where in the effective (rotated) sensor space is our touch?
                    let eff_touch_x = (ui.touch_start_x - display_offset_x) / scale;
                    let eff_touch_y = (ui.touch_start_y - display_offset_y) / scale;

                    // In 1:1 mode, we want this effective position to appear at touch_start position
                    // pan_offset shifts our view, so if we want eff_touch_x to appear at touch_start_x:
                    // display_x + pan_offset_x maps to sensor, so:
                    // touch_start_x + pan_offset_x = eff_touch_x (in effective space)
                    ui.pan_offset_x = eff_touch_x - ui.touch_start_x;
                    ui.pan_offset_y = eff_touch_y - ui.touch_start_y;

                    if crate::DEBUG {
                        debug!(
                            "Pan offset calculated: ({:.1}, {:.1})",
                            ui.pan_offset_x, ui.pan_offset_y
                        );
                    }
                }
            }

            if ui.is_dragging && ui.view_1to1 && !ui.histogram_visible {
                // Pan in 1:1 mode
                let old_pan_x = ui.pan_offset_x;
                let old_pan_y = ui.pan_offset_y;

                ui.pan_offset_x = ui.pan_offset_x - dx;
                ui.pan_offset_y = ui.pan_offset_y - dy;
                ui.touch_start_x = x;
                ui.touch_start_y = y;

                if crate::DEBUG {
                    trace!(
                        "Panning: offset ({:.1}, {:.1}) -> ({:.1}, {:.1}), delta: ({:.1}, {:.1})",
                        old_pan_x,
                        old_pan_y,
                        ui.pan_offset_x,
                        ui.pan_offset_y,
                        dx,
                        dy
                    );
                }
                return true;
            }

            false
        }
        TouchAction::Up => {
            if crate::DEBUG {
                debug!("Touch end at ({:.1}, {:.1})", x, y);
            }

            // End touch
            // Clear any pressed arrows and buttons
            let had_pressed_arrow = ui.pressed_arrow.is_some();
            let had_active_button = ui.active_button.is_some();
            let active_button_index = ui.active_button;

            if had_pressed_arrow {
                if crate::DEBUG {
                    debug!("Clearing pressed arrow: {:?}", ui.pressed_arrow);
                }
            }
            if had_active_button {
                if crate::DEBUG {
                    debug!("Clearing active button: {:?}", active_button_index);
                }
            }

            ui.pressed_arrow = None;
            ui.active_button = None;

            // Handle button release - only trigger if releasing on same button
            if let Some(button_index) = active_button_index {
                if let Some(TrackElement::Button { index }) = get_element_from_touch(ui, x, y) {
                    if index == button_index {
                        if crate::DEBUG {
                            info!("Button {} activated", button_index);
                        }
                        // Released on same button - do the action
                        ui.handle_button_press(button_index);
                    } else {
                        if crate::DEBUG {
                            debug!(
                                "Button release mismatch: pressed {}, released on {}",
                                button_index, index
                            );
                        }
                    }
                } else {
                    if crate::DEBUG {
                        debug!("Button {} released outside button area", button_index);
                    }
                }
            }

            if had_pressed_arrow || had_active_button {
                return true; // Need redraw to show unpressed state
            }

            // If touch started in margin, ignore
            if ui.touch_is_dead {
                if crate::DEBUG {
                    debug!("Touch was dead, cleaning up");
                }
                ui.touch_is_dead = false;
                return false;
            }

            let was_dragging = ui.is_dragging;
            let had_active_slider = ui.active_track.is_some();

            if was_dragging {
                if crate::DEBUG {
                    debug!("Ending drag operation");
                }
            }
            if had_active_slider {
                if crate::DEBUG {
                    debug!("Ending slider operation: track {:?}", ui.active_track);
                }
            }

            ui.is_dragging = false;

            // Clear active slider if we had one
            if had_active_slider {
                ui.active_track = None;
                return true;
            }

            // Only return to fit mode if we're in 1:1 and it was a tap (not drag or slider)
            if ui.view_1to1 && !was_dragging {
                if crate::DEBUG {
                    info!("Tap in 1:1 mode - returning to fit mode");
                }
                // Tap in 1:1 mode - return to fit
                ui.view_1to1 = false;
                ui.pan_offset_x = 0.;
                ui.pan_offset_y = 0.;
                return true;
            } else if !ui.view_1to1 && !was_dragging {
                if crate::DEBUG {
                    info!(
                        "Tap in fit mode - toggling controls (visible: {} -> {})",
                        ui.controls_visible, !ui.controls_visible
                    );
                }
                // Tap in fit mode, not on slider - toggle controls
                ui.controls_visible = !ui.controls_visible;
                return true;
            }

            false
        }
    }
}

// Write slider values to shared memory - call once after adjustment
fn update_shared_memory_for_track(ui: &mut UserInterface, track: usize) {
    match track {
        1 => {
            let focus_distance = ui.focus_slider * ui.min_focus_distance;
            ui.header[FOCUS_IDX] = focus_distance.to_bits();
            if crate::DEBUG {
                trace!(
                    "Shared memory updated: focus_distance={:.2}",
                    focus_distance
                );
            }
        }
        2 => {
            let shutter_ns =
                ui.min_shutter_ns * (ui.max_shutter_ns / ui.min_shutter_ns).powf(ui.shutter_slider);
            ui.header[SHUTTER_NS_IDX] = shutter_ns.to_bits();
            if crate::DEBUG {
                trace!("Shared memory updated: shutter_ns={:.0}", shutter_ns);
            }
        }
        3 => {
            let current_iso = ui.min_iso * (ui.max_iso / ui.min_iso).powf(ui.iso_slider);
            ui.header[ISO_IDX] = current_iso.to_bits();
            if crate::DEBUG {
                trace!("Shared memory updated: current_iso={:.0}", current_iso);
            }
        }
        4 => {
            ui.header[EXPOSURE_TIME_MS_IDX] = (ui.exposure_time_ms as f64).to_bits();
            if crate::DEBUG {
                trace!(
                    "Shared memory updated: exposure_time_ms={}",
                    ui.exposure_time_ms
                );
            }
        }
        _ => {
            // Track 0 (gain) doesn't write to shared memory, others invalid
            if crate::DEBUG && track != 0 {
                warn!("Invalid track {} for shared memory update", track);
            }
        }
    }
}

// Unified touch target enum
enum TrackElement {
    Slider { track: usize, value: f32 },
    Arrow { track: usize, increase: bool },
    Button { index: usize },
}

// Complete UI touch handling for all sliders and buttons. Handles all cases
fn get_element_from_touch(ui: &mut UserInterface, x: f32, y: f32) -> Option<TrackElement> {
    // Normalize coordinates to 0-1 range (accounting for margins)
    let norm_x = (x - ui.x_margin) / (ui.screen_run as f32 - ui.x_margin * 2.);
    let norm_y = (y - ui.y_margin) / (ui.screen_rise as f32 - ui.y_margin * 2.);

    let controls_ratio = if ui.is_fat() {
        ui_constants::CONTROLS_HEIGHT_FAT
    } else {
        ui_constants::CONTROLS_HEIGHT_SKINNY
    };

    let (horizontal, vertical) = match (ui.is_fat(), ui.device_rotation > 127) {
        (true, true) => (norm_y, norm_x),
        (true, false) => (1. - norm_y, 1. - norm_x),
        (false, true) => (1. - norm_x, norm_y),
        (false, false) => (norm_x, 1. - norm_y),
    };

    let controls_vertical = vertical / controls_ratio;

    if crate::DEBUG {
        debug!("Touch target calculation: norm=({:.3}, {:.3}), transformed=({:.3}, {:.3}), controls_ratio={:.3}", 
               norm_x, norm_y, horizontal, controls_vertical, controls_ratio);
    }

    // Determine which track was touched
    let track = (controls_vertical * ui_constants::TOTAL_TRACKS as f32) as usize;
    if crate::DEBUG {
        trace!(
            "Track calculation: vertical={:.3} * {} tracks = track {}",
            controls_vertical,
            ui_constants::TOTAL_TRACKS,
            track
        );
    }

    match track {
        // Tracks 0-4: Sliders with arrows
        0..=4 => {
            let slider_value = horizontal_to_slider(ui, horizontal);
            if crate::DEBUG {
                trace!("Track {} slider value: {:.3}", track, slider_value);
            }

            match slider_value {
                v if v < 0. => {
                    if crate::DEBUG {
                        trace!("Left arrow on track {}", track);
                    }
                    Some(TrackElement::Arrow {
                        track,
                        increase: false,
                    })
                }
                v if v > 1. => {
                    if crate::DEBUG {
                        trace!("Right arrow on track {}", track);
                    }
                    Some(TrackElement::Arrow {
                        track,
                        increase: true,
                    })
                }
                v => {
                    if crate::DEBUG {
                        trace!("Slider on track {} at value {:.3}", track, v);
                    }
                    Some(TrackElement::Slider { track, value: v })
                }
            }
        }
        // Tracks 5-6: Buttons
        5..=6 => {
            let button_width = if ui.is_fat() {
                ui_constants::FAT_BUTTON_WIDTH
            } else {
                ui_constants::SKINNY_BUTTON_WIDTH
            };
            let button_index = match horizontal {
                h if h < button_width => (track - 5) * 2, // Left third: buttons 0, 2
                h if h > 1. - button_width => (track - 5) * 2 + 1, // Right third: buttons 1, 3
                _ => {
                    if crate::DEBUG {
                        trace!("Button touch in middle third (no button)");
                    }
                    return None; // Middle third: no button
                }
            };
            if crate::DEBUG {
                trace!(
                    "Button {} touched (track {}, horizontal: {:.3})",
                    button_index,
                    track,
                    horizontal
                );
            }
            Some(TrackElement::Button {
                index: button_index,
            })
        }
        _ => {
            // Check if touch is in calibration button area (top-left corner)
            let ar = (ui.screen_run as f32 - ui.x_margin * 2.)
                .max(ui.screen_rise as f32 - ui.y_margin * 2.)
                / (ui.screen_run as f32 - ui.x_margin * 2.)
                    .min(ui.screen_rise as f32 - ui.y_margin * 2.);

            let (h_scale, v_scale) = if ui.is_fat() { (ar, 1.) } else { (1., ar) };
            if horizontal * h_scale < CALIBRATION_BUTTON_SIZE
                && (1. - vertical) * v_scale < CALIBRATION_BUTTON_SIZE
            {
                if crate::DEBUG {
                    debug!(
                        "Calibration button touched at ({:.3}, {:.3})",
                        horizontal, vertical
                    );
                }
                Some(TrackElement::Button { index: 4 }) // Calibration is button index 4
            } else {
                if crate::DEBUG {
                    debug!(
                        "Touch outside controls ({:.3}, {:.3})",
                        horizontal, vertical
                    );
                }
                None
            }
        }
    }
}

pub fn horizontal_to_slider(ui: &UserInterface, horizontal: f32) -> f32 {
    let result = if ui.is_fat() {
        let scaled_label_width = ui_constants::LABEL_WIDTH * ui.screen_aspect;
        (horizontal - scaled_label_width) / (1. - scaled_label_width * 2.)
    } else {
        (horizontal - ui_constants::LABEL_WIDTH) / (1. - ui_constants::LABEL_WIDTH * 2.)
    };
    if crate::DEBUG {
        trace!(
            "horizontal_to_slider: {:.3} -> {:.3} (fat: {})",
            horizontal,
            result,
            ui.is_fat()
        );
    }
    result
}

pub fn slider_to_horizontal(ui: &UserInterface, slider_pos: f32) -> f32 {
    let result = if ui.is_fat() {
        let scaled_label_width = ui_constants::LABEL_WIDTH * ui.screen_aspect;
        slider_pos * (1. - scaled_label_width * 2.) + scaled_label_width
    } else {
        slider_pos * (1. - ui_constants::LABEL_WIDTH * 2.) + ui_constants::LABEL_WIDTH
    };
    if crate::DEBUG {
        trace!(
            "slider_to_horizontal: {:.3} -> {:.3} (fat: {})",
            slider_pos,
            result,
            ui.is_fat()
        );
    }
    result
}

/// True if (x, y) is within the top-right counter block (the S/I/F/format stack). Used to route a tap there to the save-format cycle instead of the margin dead-zone. Tests the union of `counter_areas` (kept current by save_counter_areas) with a small touch padding.
fn point_in_counter_block(ui: &UserInterface, x: f32, y: f32) -> bool {
    let pad = 8.0;
    for &(x0, y0, x1, y1) in ui.counter_areas.iter() {
        if x1 <= x0 || y1 <= y0 {
            continue; // empty/uninitialised area
        }
        if x >= x0 as f32 - pad
            && x <= x1 as f32 + pad
            && y >= y0 as f32 - pad
            && y <= y1 as f32 + pad
        {
            return true;
        }
    }
    false
}
