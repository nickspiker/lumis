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
            &ui.calibration_button_buffer,
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
                // Track 1 is FOCUS. A fixed-focus lens (e.g. the 0.5x ultrawide) reports min focus
                // distance 0, so focus can't be controlled - draw that track greyed (and the touch
                // handler ignores it) instead of a live black/white slider.
                let focus_disabled = track == 1 && ui.min_focus_distance <= 0.0;
                // Greyed slider colours when disabled; normal white(left)/black(right) otherwise.
                let (lr, lg, lb, rr, rg, rb) = if focus_disabled {
                    (90u8, 90, 90, 60, 60, 60)
                } else {
                    (255u8, 255, 255, 0, 0, 0)
                };

                let (start_x, start_y) = user_to_screen(ui, slider_start, track_center);
                let (pos_x, pos_y) = user_to_screen(ui, slider_horizontal, track_center);
                let (end_x, end_y) = user_to_screen(ui, slider_end, track_center);

                for y in start_y.min(pos_y) as usize..=start_y.max(pos_y) as usize {
                    for x in start_x.min(pos_x) as usize..=start_x.max(pos_x) as usize {
                        let idx = (y * screen_buffer.stride as usize + x) * 3;
                        pixels[idx] = lr;
                        pixels[idx + 1] = lg;
                        pixels[idx + 2] = lb;
                    }
                }
                for y in end_y.min(pos_y) as usize..=end_y.max(pos_y) as usize {
                    for x in end_x.min(pos_x) as usize..=end_x.max(pos_x) as usize {
                        let idx = (y * screen_buffer.stride as usize + x) * 3;
                        pixels[idx] = rr;
                        pixels[idx + 1] = rg;
                        pixels[idx + 2] = rb;
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
        // Level indicator: three circles (slot 2 when a colour cal is active, else centred).
        draw_level_indicator(ui, pixels, screen_buffer.stride as usize, screen_buffer.height as u32);
        // Slot 1: colour-calibration readout (target type/serial/life/UV/IR/warning), only when a cal is active.
        draw_cal_info(ui, pixels, screen_buffer.stride as usize, screen_buffer.height as u32);
    } // End of controls_visible block
}

// Three-circle level / horizon indicator, centred horizontally at the top of the screen.
// Geometry: a rigid horizontal triad (outer - center - outer). ROLL spins the whole triad about its centre, counter-spun x4 (the triad is symmetric so 4 visual turns per one phone turn; the fast counter-rotation makes small roll errors obvious). PITCH offsets the centre circle along the triad's perpendicular (up toward sky, down toward ground) and pushes the two outer circles the opposite way.
// Colour: the CENTRE circle reports PITCH - green when level, ramping through yellow to red as it tilts. The two OUTER circles report ROLL - green on a 45deg cardinal, yellow halfway, red at the 22.5deg midpoints.
// Rational ease, one knob: f(t) = t*(1+k)/(1 + k*t), t in 0..1. Finite slope (1+k) at 0 - steep near the target, easing flat toward t=1 - with exact endpoints f(0)=0, f(1)=1. Unlike cbrt/sqrt (infinite slope at 0, twitchy at the sweet spot) the slope is bounded and tunable: bigger k = more sensitivity near aligned (tinier green sliver). Hard limit k > -1 (at k <= -1 the denominator can hit zero and blow up); k >= 0 is the useful range.
const ROLL_EASE_K: f32 = 4.0; // roll: drives both the colour ramp AND the spin snap
const PITCH_EASE_K: f32 = 16.0; // pitch: colour only (tighter green sliver than roll)
const SPACING_SPREAD: f32 = 6.0; // extra centre-to-outer spacing (in radii) added as the centre dot goes red; 0 = always tangent
fn level_ease(t: f32, k: f32) -> f32 {
    t * (1.0 + k) / (1.0 + k * t)
}

fn draw_level_indicator(ui: &mut UserInterface, pixels: &mut [u8], stride: usize, height: u32) {
    let short = ui.screen_run.min(ui.screen_rise) as f32;

    // Layout sizes scaled to the screen's short edge so it looks right on any device/orientation (half the original size).
    let radius = short * 0.006; // circle radius
    let pitch_gain = short * 0.03; // how far a full tilt pushes the circles vertically
    // Anchor via user_to_screen (held-orientation aware). When a colour calibration is active the top row is a 4-slot grid - slot 0 target, slot 1 cal info, slot 2 level, slot 3 counters - so the indicator sits at slot 2's centre (x=2/3). Uncalibrated, it's centred (x=1/2) with slots 1/2 unused.
    let anchor_x = if ui.calibration_info.load().is_some() { 2.0 / 3.0 } else { 0.5 };
    let (cx, cy) = user_to_screen(ui, anchor_x, 0.04);

    // ROLL drives the triad's orientation (counter-spun x4) PLUS the held-orientation offset so the triad's base frame matches the rotated UI (otherwise landscape reads 90deg off). device_rotation is degrees (0/90/180/270); add it in radians so portrait and landscape share the same "level" reference.
    let ui_offset = (ui.device_rotation as f32).to_radians();
    // Ease the roll toward the nearest 45deg cardinal so the triad SNAPS into alignment (and stays near it longer): split roll into nearest-cardinal + signed remainder, ease the remainder's magnitude (steeper near the cardinal), recombine. Same rational ease as the colour, so spin and colour accelerate together.
    let card = 45.0f32.to_radians();
    let nearest = (ui.roll / card).round() * card;
    let rem = ui.roll - nearest; // signed, within +/- 22.5deg
    let eased_roll = nearest + rem.signum() * level_ease(rem.abs() / (card * 0.5), ROLL_EASE_K) * (card * 0.5);
    let spin = -eased_roll * 4.0 + ui_offset;
    let (s, c) = (spin.sin(), spin.cos());

    // PITCH folds to 90deg cardinals (green at 0, +90, -90), just like roll folds to 45deg. pitch_rem is the signed distance to the nearest 90deg multiple (+/- 45deg) - the alignment error that drives BOTH the dot position and its colour, so the dot returns to centre + green at every cardinal and is maximally offset + red at the +/-45deg midpoints.
    let pcard = 90.0f32.to_radians();
    let pitch_rem = ui.pitch - (ui.pitch / pcard).round() * pcard; // +/- 45deg, signed
    // Eased pitch error magnitude (0 at a cardinal = green, 1 at the +/-45deg worst = red). Same value drives the centre dot's COLOUR and the triad SPACING - so the outer dots kiss the centre (tangent) when green and spread out as it reds.
    let pitch_t = level_ease(pitch_rem.abs() / (pcard * 0.5), PITCH_EASE_K);
    // Spacing: 2*radius (tangent, no gap) at green, opening up by SPACING_SPREAD*radius more as the centre dot goes red.
    let spacing = radius * (2.0 + pitch_t * SPACING_SPREAD);
    // Position offset: the SAME eased magnitude as the colour/gap (pitch_t), signed by which way we're off the cardinal, so the vertical motion snaps near level exactly like the colour does. Positive remainder = tilted past the cardinal toward the sky -> centre moves UP. Direction is handled entirely by `perp` (which carries ui_offset, so it already rotates correctly per held orientation) - no extra orientation flip here (an earlier one double-flipped 180/270).
    let pitch_off = pitch_rem.signum() * pitch_t * pitch_gain;

    // The triad's base line direction is horizontal (1,0) rotated by `spin`; outer circles sit at +/-spacing along it. The line carries the roll spin so it tilts as you roll.
    let line = (c, s); // unit vector along the triad

    // The PITCH axis must always be the user's true screen-up, regardless of held orientation - derived directly from user_to_screen (the vector from a lower point to the anchor), NOT from the spun perpendicular (which mixed in the roll spin and orientation and flipped wrong at 180/270). Normalise it; pitch then pushes the centre along +up and the outers along -up.
    let (bx, by) = user_to_screen(ui, anchor_x, 0.08);
    let (ux, uy) = (cx - bx, cy - by);
    let ulen = (ux * ux + uy * uy).sqrt().max(1e-6);
    let up = (ux / ulen, uy / ulen); // unit "toward the user's top of screen"

    // Centre circle: shifted toward the sky by pitch (UP * pitch_off).
    let center = (cx + up.0 * pitch_off, cy + up.1 * pitch_off);
    // Outer circles: along the spun line at +/-spacing, shifted the OPPOSITE way in pitch.
    let left = (cx - line.0 * spacing - up.0 * pitch_off, cy - line.1 * spacing - up.1 * pitch_off);
    let right = (cx + line.0 * spacing - up.0 * pitch_off, cy + line.1 * spacing - up.1 * pitch_off);

    // green -> yellow -> red ramp over t in 0..1 (0 = on target = green, 1 = worst = red). Both channels overshoot to 510 and the f32->u8 saturating cast clamps to 255: red = t*510 (0 -> 255 by the midpoint), green = (1-t)*510 (255 until the midpoint -> 0). At t=0.5 both read 255 = bright yellow.
    let ramp = |t: f32| -> (u8, u8, u8) { ((t * 510.0) as u8, ((1.0 - t) * 510.0) as u8, 0u8) };

    // Centre = PITCH: green at the 0/+90/-90 cardinals, red at the +/-45deg midpoints. Reuses pitch_t (the same eased error that drives the spacing) so colour and spread stay locked together.
    let center_col = ramp(pitch_t);
    // Outer = ROLL: colour falls straight out of the SAME rotation math as the spin - `rem` is the signed alignment error (how far the triad is from its nearest cardinal, +/- 22.5deg). Normalise to 0..1 and ease: 0 = aligned = green, 1 = worst (22.5deg) = red. One source of truth for spin and colour.
    let outer_col = ramp(level_ease(rem.abs() / (card * 0.5), ROLL_EASE_K));

    fill_circle(pixels, stride, left.0, left.1, radius, outer_col);
    fill_circle(pixels, stride, right.0, right.1, radius, outer_col);
    fill_circle(pixels, stride, center.0, center.1, radius, center_col);

    // Numeric readouts flanking the dots: ROLL on the left, PITCH on the right. Both show the signed error from the nearest cardinal (the same `rem`/`pitch_rem` that drive colour) in degrees - how far off level, which is what the user is correcting. Each in its axis's own colour so the number matches its dot.
    let roll_deg = rem.to_degrees();
    let pitch_deg = pitch_rem.to_degrees();
    let text_size = radius * 4.5;
    let rot = ui.device_rotation as u16;
    // Place the text in USER space (left/right of the anchor) via user_to_screen, so the held-orientation flip/swap keeps it horizontal beside the dots - offsetting raw screen-x put it top/bottom in portrait. gap_frac is a fraction of the content width, clear of the triad's widest spread.
    let gap_frac = 0.10;
    let (rx, ry) = user_to_screen(ui, anchor_x - gap_frac, 0.04);
    let (px, py) = user_to_screen(ui, anchor_x + gap_frac, 0.04);
    ui.text_renderer.draw_text_right(pixels, stride as u32, height, &format!("{:+.2}", roll_deg), rx, ry, text_size, 400, outer_col.0, outer_col.1, outer_col.2, rot);
    ui.text_renderer.draw_text_left(pixels, stride as u32, height, &format!("{:+.2}", pitch_deg), px, py, text_size, 400, center_col.0, center_col.1, center_col.2, rot);
}

// Slot 1 of the calibrated 4-slot top row (target | cal info | level | counts): the colour-calibration readout - target type/serial, target life, UV + IR content, and any warning. Drawn only when a colour cal is active (calibration_info is Some); centred horizontally on slot 1 (x = 1/3), stacked vertically, rotated with the held UI. Colours mirror chameleon's terminal report: life green->red by remaining %, UV violet, IR per-channel R/G/B, warnings amber.
fn draw_cal_info(ui: &mut UserInterface, pixels: &mut [u8], stride: usize, height: u32) {
    let info = ui.calibration_info.load();
    let info = match info.as_ref() {
        Some(i) => i.clone(),
        None => return,
    };
    let short = ui.screen_run.min(ui.screen_rise) as f32;
    let size = short * 0.018; // line text height
    let line_step = 0.035; // vertical spacing between lines, in user-space fraction
    let rot = ui.device_rotation as u16;
    let cx_frac = 1.0 / 3.0; // slot 1 centre

    // Target life colour: green (fresh) -> red (worn), matching the terminal's tl_r/tl_g ramp.
    let life = info.life.clamp(0.0, 1.0);
    let life_col = (((1.0 - life) * 128.0 + 127.0) as u8, (life * 128.0 + 127.0) as u8, 127u8);

    // (text, r, g, b) per line. IR is per-channel so we show it as one R/G/B-tinted triple line.
    let lines: [(String, u8, u8, u8); 5] = [
        (format!("TYPE {} / SN {}", info.target_type, info.serial), 0xE0, 0xE0, 0xE0),
        (format!("life {:.0}%", life * 100.0), life_col.0, life_col.1, life_col.2),
        (format!("UV {:.0}%", (info.uv.max(0.0)) * 100.0), 0x9F, 0x7F, 0xFF),
        (
            format!("IR {:.0}/{:.0}/{:.0}%", info.ir[0] * 100.0, info.ir[1] * 100.0, info.ir[2] * 100.0),
            0xE0, 0xC0, 0xC0,
        ),
        // Warning line (amber) if present, else gamma as a quiet status line.
        if info.warning.trim().is_empty() {
            (format!("gamma {:.2}", info.gamma), 0xA0, 0xA0, 0xA0)
        } else {
            ("CHECK TARGET".to_string(), 0xFF, 0xC0, 0x00)
        },
    ];

    let y0 = 0.03;
    for (i, (text, r, g, b)) in lines.iter().enumerate() {
        let (tx, ty) = user_to_screen(ui, cx_frac, y0 + i as f32 * line_step);
        ui.text_renderer.draw_text_center(pixels, stride as u32, height, text, tx, ty, size, 400, *r, *g, *b, rot);
    }
}

// Anti-aliased filled circle in screen pixel space, alpha-blended over the live preview. No sqrt: like photon's draw_filled_circle, we compare SQUARED distances - full inside r_inner2, zero outside r_outer2, linear AA across the one-pixel edge band between them (coverage = (r_outer2 - dist2) / (r_outer2 - r_inner2)).
fn fill_circle(pixels: &mut [u8], stride: usize, cx: f32, cy: f32, radius: f32, color: (u8, u8, u8)) {
    // No bounds checks: the indicator lives at screen top-centre with a tiny radius, so its bounding box is always well inside the buffer - the pixels are mathematically in-bounds.
    let r_out = radius + 0.5;
    let r_in = radius - 0.5;
    let r_out2 = r_out * r_out;
    let r_in2 = r_in * r_in;
    let edge = (r_out2 - r_in2).max(1.0); // edge-band width in squared-distance units; >=1 avoids a /0 at radius<=0.5 (math constant for the AA divide, not a value guard)
    let x0 = (cx - r_out).floor() as usize;
    let x1 = (cx + r_out).ceil() as usize;
    let y0 = (cy - r_out).floor() as usize;
    let y1 = (cy + r_out).ceil() as usize;
    for y in y0..=y1 {
        let dy = y as f32 + 0.5 - cy;
        let dy2 = dy * dy;
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dist2 = dx * dx + dy2;
            if dist2 > r_out2 {
                continue; // outside the disc - not this pixel's geometry
            }
            // AA coverage weight in 0..1 (blend weight, not a value/bounds guard): 1 inside r_in2, ramping to 0 at r_out2.
            let cov = if dist2 <= r_in2 { 1.0 } else { (r_out2 - dist2) / edge };
            let idx = (y * stride + x) * 3;
            pixels[idx] = (color.0 as f32 * cov + pixels[idx] as f32 * (1.0 - cov)) as u8;
            pixels[idx + 1] = (color.1 as f32 * cov + pixels[idx + 1] as f32 * (1.0 - cov)) as u8;
            pixels[idx + 2] = (color.2 as f32 * cov + pixels[idx + 2] as f32 * (1.0 - cov)) as u8;
        }
    }
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
