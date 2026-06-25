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

/// Apply the 3x3 camera->Rec.2020 display matrix (magic_9_display) to a linear RGB triple. Row-major: out_r = m0*r + m1*g + m2*b, etc. Done in linear light, before the sqrt display encode.
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

    // While the feed is frozen for calibration (and the overlay held), force full draws: the image counter is frozen so the normal new-frame trigger won't fire, and we need the frozen frame + overlay to paint, then a clean repaint on dismiss.
    let hold_present = ui.calibration_hold.load().is_some() || ui.frozen_image_counter.is_some();
    if hold_present || ui.previous_hold_present {
        full_draw = true;
    }
    ui.previous_hold_present = hold_present;

    // Dark-frame calibration screen: frames arrive ~16s apart but the stats update on a ~1.5s throttle,
    // so force a full draw every render-loop tick to keep the live stats refreshing smoothly. The
    // post-finalize result view also forces a draw so it paints promptly when finalize completes.
    if (ui.header[FLAGS_IDX] & (CALIBRATING_BIT | CAL_SHOW_RESULT_BIT)) != 0 {
        full_draw = true;
    }

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
    use crate::shared_memory::{CALIBRATING_BIT, CAL_SHOW_RESULT_BIT, FLAGS_IDX};
    // After finalize: show the captured dark frame, brightened (gamma ~4) so the noise is visible, until
    // the user taps to return to the menu. Checked before the live-stats screen.
    if (ui.header[FLAGS_IDX] & CAL_SHOW_RESULT_BIT) != 0 {
        draw_calibration_result(ui, pixels, buffer);
        return;
    }
    // Dark-frame calibration capture has its own minimal screen: just progress stats and an
    // End/Finalize button - no live image, no camera controls. Render it and return.
    if (ui.header[FLAGS_IDX] & CALIBRATING_BIT) != 0 {
        draw_calibration_screen(ui, pixels, buffer);
        return;
    }

    // Draw the camera image (frozen during calibration). When a calibration hold is active, the in-place live overlay is composited per-pixel inside the fit-to-screen loop.
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

// Minimal dark-frame calibration screen: black background, live progress stats, and a FINALIZE button.
// No image, no camera controls - the user just watches the convergence and taps FINALIZE when satisfied
// (which sets CAL_FINALIZE_BIT; the integrator then averages, writes the cal to disk, and stops).
fn draw_calibration_screen(
    ui: &mut UserInterface,
    pixels: &mut [u8],
    buffer: &ANativeWindow_Buffer,
) {
    use crate::shared_memory::*;
    let w = buffer.stride as u32;
    let h = buffer.height as u32;
    // Clear to black.
    for p in pixels.iter_mut() {
        *p = 0;
    }

    // Read the published progress stats.
    let is_dark = (ui.header[FLAGS_IDX] & CAL_IS_DARK_BIT) != 0;
    let frames = ui.header[CAL_FRAME_COUNT_IDX];
    let elapsed_ms = ui.header[CAL_ELAPSED_MS_IDX];
    let correlation = f64::from_bits(ui.header[CAL_CORRELATION_IDX]);
    let mean = f64::from_bits(ui.header[CAL_MEAN_IDX]);
    let noise = f64::from_bits(ui.header[CAL_NOISE_IDX]);

    // Record one noise sample per new frame (the stats publish on a throttle, so dedup on frame count).
    // A reset (frame count went backwards, e.g. a new capture) clears the history.
    if frames < ui.cal_noise_last_frame {
        ui.cal_noise_history.clear();
    }
    if frames > 0 && frames != ui.cal_noise_last_frame {
        ui.cal_noise_history.push(noise as f32);
        ui.cal_noise_last_frame = frames;
        // Cap the history so the plot stays bounded on very long runs (keep the most recent points).
        if ui.cal_noise_history.len() > 2048 {
            ui.cal_noise_history.remove(0);
        }
    }

    let cx = w as f32 / 2.;
    let line_h = h as f32 * 0.055;
    let size = line_h * 0.6;
    let y0 = h as f32 * 0.16;

    // Convergence colour cue: red->green as it approaches 1.0 (clean fixed pattern).
    let cg = (correlation.clamp(0., 1.) * 255.) as u8;
    let cr = 255 - cg;
    let secs = elapsed_ms / 1000;

    // (text, r, g, b) per line; the title gets a half-line gap after it (index 0).
    let lines: [(String, u8, u8, u8); 6] = [
        (
            if is_dark { "DARK CALIBRATION".into() } else { "BIAS CALIBRATION".into() },
            0xFF,
            0xC0,
            0x40,
        ),
        (format!("frames: {}", frames), 0xE0, 0xE0, 0xE0),
        (format!("elapsed: {}m {}s", secs / 60, secs % 60), 0xE0, 0xE0, 0xE0),
        (format!("convergence: {:.3}", correlation), cr, cg, 0x40),
        (format!("mean: {:.0}", mean), 0xE0, 0xE0, 0xE0),
        (format!("noise: {:.0}", noise), 0xE0, 0xE0, 0xE0),
    ];
    for (i, (text, r, g, b)) in lines.iter().enumerate() {
        // Extra half-line gap below the title.
        let y = y0 + i as f32 * line_h + if i > 0 { line_h * 0.5 } else { 0. };
        ui.text_renderer
            .draw_text_center(pixels, w, h, text, cx, y, size, 0, *r, *g, *b, 0);
    }

    // Noise-over-time graph: the ~1/sqrt(N) decrease. Auto-scaled Y to the data range so the curve fills
    // the plot and the flattening is obvious; the user watches it level off and taps FINALIZE when happy.
    let gx0 = (w as f32 * 0.12) as usize;
    let gx1 = (w as f32 * 0.88) as usize;
    let gy0 = (h as f32 * 0.50) as usize;
    let gy1 = (h as f32 * 0.72) as usize;
    // Plot border (dim grey).
    for x in gx0..gx1 {
        for &yy in &[gy0, gy1 - 1] {
            let off = (yy * w as usize + x) * 3;
            if off + 2 < pixels.len() {
                pixels[off] = 0x40;
                pixels[off + 1] = 0x40;
                pixels[off + 2] = 0x40;
            }
        }
    }
    let hist = &ui.cal_noise_history;
    if hist.len() >= 2 {
        let nmin = hist.iter().cloned().fold(f32::MAX, f32::min);
        let nmax = hist.iter().cloned().fold(f32::MIN, f32::max);
        let range = (nmax - nmin).max(1e-6);
        let gw = (gx1 - gx0) as f32;
        let gh = (gy1 - gy0) as f32;
        let plot_x = |i: usize| gx0 as f32 + gw * i as f32 / (hist.len() - 1) as f32;
        // Higher noise = higher on screen (smaller y). Map value -> y within the plot box.
        let plot_y = |v: f32| gy1 as f32 - gh * (v - nmin) / range;
        // Draw line segments between consecutive points (simple DDA), in green.
        for i in 0..hist.len() - 1 {
            let (x0, y0p) = (plot_x(i), plot_y(hist[i]));
            let (x1, y1p) = (plot_x(i + 1), plot_y(hist[i + 1]));
            let steps = ((x1 - x0).abs().max((y1p - y0p).abs())).ceil().max(1.0) as usize;
            for s in 0..=steps {
                let t = s as f32 / steps as f32;
                let px = (x0 + (x1 - x0) * t) as usize;
                let py = (y0p + (y1p - y0p) * t) as usize;
                if px < w as usize && py < h as usize {
                    let off = (py * w as usize + px) * 3;
                    if off + 2 < pixels.len() {
                        pixels[off] = 0x40;
                        pixels[off + 1] = 0xFF;
                        pixels[off + 2] = 0x60;
                    }
                }
            }
        }
    }

    // FINALIZE button - the menu's rounded teal style. Rect is shared with the touch hit-test.
    let rect = calibration_finalize_rect(w, h);
    draw_fancy_button(pixels, w as usize, h, rect, "FINALIZE", false, &mut ui.text_renderer);
}

// The finalized dark frame, brightened so the noise is visible: fit-to-screen, mean-subtracted (so the
// brighter half of pixels - hot pixels and high-noise excursions - show against the frame's own centre),
// auto-scaled to the positive spread, and gamma-2 (single sqrt) stretched. A tap exits (handled in
// touch.rs). If this still looks flat black, the capture itself is suspect (e.g. sensor clamping at max ISO).
fn draw_calibration_result(
    ui: &mut UserInterface,
    pixels: &mut [u8],
    buffer: &ANativeWindow_Buffer,
) {
    let sw = buffer.stride as usize;
    let sh = buffer.height as usize;
    let iw = ui.sensor_x_size;
    let ih = ui.sensor_y_size;
    for p in pixels.iter_mut() {
        *p = 0;
    }
    if iw == 0 || ih == 0 {
        return;
    }
    // Fit-to-screen scale (preserve aspect), centred.
    let scale = (sw as f32 / iw as f32).min(sh as f32 / ih as f32);
    let dw = (iw as f32 * scale) as usize;
    let dh = (ih as f32 * scale) as usize;
    let off_x = (sw - dw) / 2;
    let off_y = (sh - dh) / 2;
    // Simple, honest display: subtract the mean, scale by the FIXED white-black range (NOT the data's
    // own spread), gamma 2. No per-frame auto-scaling - that was the bug: dividing by each frame's own
    // max excursion cranked the gain on a low-signal BIAS frame so it looked brighter than a 16s DARK,
    // and stacked with gamma 4 it turned a genuinely near-black frame almost white. With a fixed range a
    // near-black frame correctly LOOKS near-black, and bias is dimmer than dark, as physics demands.
    let npix = iw * ih;
    let mut sum = 0.0f64;
    for i in 0..npix {
        sum += ui.image_buffer[i] as f64;
    }
    let mean = (sum / npix as f64) as f32;
    // Fixed display range: white - black (frame-independent), so brightness is comparable across captures.
    let span = (65535.0 - ui.raw_black_level as f32).max(1.0);
    for dy in 0..dh {
        let sy = (dy as f32 / scale) as usize;
        for dx in 0..dw {
            let sx = (dx as f32 / scale) as usize;
            let raw = ui.image_buffer[(sy * iw + sx).min(npix - 1)] as f32;
            let v = ((raw - mean) / span).max(0.0); // mean-subtracted, fixed-range normalised
            // gamma 4 (double sqrt): a dark frame IS near-black, so lift it hard to make the noise
            // structure visible. This was never the brightness bug - the bug was stacking it on the
            // per-frame auto-scale (now removed). On a FIXED range, gamma 4 lifts both bias and dark
            // for inspection while keeping bias correctly dimmer than dark.
            let b = (v.sqrt().sqrt() * 255.0).min(255.0) as u8;
            let off = ((off_y + dy) * sw + (off_x + dx)) * 3;
            if off + 2 < pixels.len() {
                pixels[off] = b;
                pixels[off + 1] = b;
                pixels[off + 2] = b;
            }
        }
    }
}

// Draw a rounded teal button (same radial-shaded style as the main menu's Back/Calibrate buttons) into
// `pixels` within the rect [x0,x1) x [y0,y1), with a centred label. Ported from MainMenu::draw_label_button
// so the calibration FINALIZE button matches the menu's look. `stride` is the buffer row width in pixels.
fn draw_fancy_button(
    pixels: &mut [u8],
    stride: usize,
    height: u32,
    rect: (u32, u32, u32, u32),
    label: &str,
    pressed: bool,
    text_renderer: &mut TextRenderer,
) {
    let (x0, y0, x1, y1) = (rect.0 as i32, rect.1 as i32, rect.2 as i32, rect.3 as i32);
    let bh = (y1 - y0) as f32;
    let button_margin = 3.0 / bh;
    let highlight: f32 = if pressed { 0.1 } else { 0.0 };
    let x_center = (x0 + x1) / 2;
    let y_center = (y0 + y1) / 2;
    let mut shade = |px: i32, py: i32, xw: f32, yw: f32| {
        let weight = xw * xw * xw * xw * xw + yw * yw * yw * yw * yw;
        let mut wc = weight * weight * weight * 8.;
        wc = wc * wc * wc;
        wc = 1. - (wc - 0.5).abs() * (1.65 - highlight);
        let mut wg = weight * weight * weight * 5.;
        wg = wg * wg * wg;
        wg = 1. - (wg - 0.5).abs() * (1.8 - highlight);
        let mut wa = weight * weight * weight * 5.;
        wa = wa * wa * wa;
        wa = (wa - 0.04).abs() * (1.75 - highlight);
        let offset = (py as usize * stride + px as usize) * 3;
        if offset + 2 >= pixels.len() {
            return;
        }
        // Teal: weak red, strong green+blue.
        pixels[offset] = (pixels[offset] as f32 * wa.min(1.)) as u8;
        pixels[offset + 1] = (wg.max(0.) * 0xE0 as f32 + pixels[offset + 1] as f32 * wa.min(1.)) as u8;
        pixels[offset + 2] = (wc.max(0.) * 0x100 as f32 + pixels[offset + 2] as f32 * wa.min(1.)) as u8;
    };
    for py in y0..y_center {
        for px in x0..x_center {
            shade(px, py, (1. - (px - x0) as f32 * button_margin).max(0.), (1. - (py - y0) as f32 * button_margin).max(0.));
        }
        for px in x_center..x1 {
            shade(px, py, (1. - (x1 - px - 1) as f32 * button_margin).max(0.), (1. - (py - y0) as f32 * button_margin).max(0.));
        }
    }
    for py in y_center..y1 {
        for px in x0..x_center {
            shade(px, py, (1. - (px - x0) as f32 * button_margin).max(0.), (1. - (y1 - py - 1) as f32 * button_margin).max(0.));
        }
        for px in x_center..x1 {
            shade(px, py, (1. - (x1 - px - 1) as f32 * button_margin).max(0.), (1. - (y1 - py - 1) as f32 * button_margin).max(0.));
        }
    }
    let (a, r, g, b) = if pressed { (300u16, 64u8, 255u8, 255u8) } else { (200u16, 0u8, 200u8, 255u8) };
    text_renderer.draw_text_center(
        pixels,
        stride as u32,
        height,
        label,
        (x0 + x1) as f32 / 2.,
        y0 as f32 + bh / 2.,
        bh * 0.5,
        a,
        r,
        g,
        b,
        0,
    );
}

// The on-screen rectangle (x0,y0,x1,y1) of the calibration FINALIZE button, in buffer pixels. Shared
// by the renderer and the touch hit-test so they can't drift apart.
pub fn calibration_finalize_rect(w: u32, h: u32) -> (u32, u32, u32, u32) {
    let bw = (w as f32 * 0.6) as u32;
    let bh = (h as f32 * 0.08) as u32;
    let x0 = (w - bw) / 2;
    let y0 = (h as f32 * 0.78) as u32;
    (x0, y0, x0 + bw, y0 + bh)
}

// The original image-drawing body: histogram if visible, else the camera image.
fn draw_camera_or_histogram(
    ui: &mut UserInterface,
    pixels: &mut [u8],
    buffer: &ANativeWindow_Buffer,
) {
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
        // Quad-Bayer (Tetracell) mode: the CFA is a 4x4 tile of four 2x2 same-colour clusters, so a 2x2 block is a single colour and naive debayer renders grey. When set, both preview paths resolve colour from the 4x4 quad tile instead. Matches ui.rs's `is_quad` usage.
        let is_quad = ui.header[crate::shared_memory::QUAD_BAYER_IDX] != 0;
        // Per-2x2-cell colour (0=R,1=G,2=B) of the base Bayer pattern, indexed row-major (idx = row*2 + col). The fit-to-screen Average bin uses this to classify each source pixel: for standard Bayer it is the colour of each pixel of the 2x2 cell; for quad-Bayer it is the colour of each 2x2 CLUSTER of the 4x4 tile.
        let base_2x2 = |bayer_pattern: u32| -> [usize; 4] {
            match bayer_pattern {
                0 => [0, 1, 1, 2], // RGGB
                1 => [1, 0, 2, 1], // GRBG
                2 => [1, 2, 0, 1], // GBRG
                3 => [2, 1, 1, 0], // BGGR
                _ => [0, 1, 1, 2],
            }
        };

        // While calibration holds the feed, render from the snapshot taken at freeze time (avg at 0, diff at pixel_count) so the camera thread can't change it. Otherwise read the current live slot.
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
            // 1:1 pixel view (zoomed in). Each screen pixel maps to one sensor pixel via the pan offset + rotation. Average mode is colour: cheap per-pixel 2x2-block debayer + display matrix + sqrt (fast enough for the per-frame UI thread — the high-quality RCD demosaic is reserved for saving, not the live preview, matching how camera apps run a light demosaic live and the good one only on capture). Difference/Motion are monochrome magnitudes, left unmixed.
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
                            let last = ui.sensor_x_size * ui.sensor_y_size - 1;
                            // Black-level subtract, mirroring the fit-to-screen path EXACTLY so both clip indicators match: controls visible -> white-clip zeros over-threshold channels (-> dark/false colour) AND black-clip uses a WRAPPING sub so a pixel below black_level underflows to a huge u16 -> renders WHITE (the crushed-shadow indicator). Controls hidden -> saturating sub (no overlay).
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
                            // 1:1 zoomed view is raw-direct greyscale: no debayer at all. Read THIS exact sensor pixel, black-subtract (sub, with the same clip indicator as every other path), gain, sqrt, and write it to all three channels. Showing the raw mosaic as luma at 1:1 lets you inspect per-pixel sensor data (CFA pattern, hot pixels, noise) without any colour interpolation. No display matrix here: it mixes channels and would tint a grey value.
                            let v = sub(raw_average[idx.min(last)]) * scale_avg;
                            // No clamps: `as u8` saturates >255 -> 255 and negative -> sqrt = NaN -> 0 (black), the desired output.
                            let l = v.sqrt() as u8;
                            pixels[dst_idx] = l;
                            pixels[dst_idx + 1] = l;
                            pixels[dst_idx + 2] = l;
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
                            if ui.controls_visible && pixel_value > ui_constants::CLIPPING_THRESHOLD
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
            // In-place calibration overlay (live_overlay): held until tap, composited per sensor pixel below. (min_x, min_y, ov_w, ov_h are in chameleon's tstats space = 2x the scan-input frame; see the composite site for the full-res mapping. rgba_f32 in display-linear / terminal space, white=65535.)
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
                            // Clipping overlay (controls visible): white-clip zeros over-threshold channels (-> dark/false colour), and black-clip uses a WRAPPING subtract so a pixel below black_level underflows to a huge value -> renders WHITE. That flash-white-on-crushed-shadow IS the black-clip indicator. Controls hidden: plain saturating subtract (no overlay).
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

                    // Diff/Motion only: pick R, G, B from the 2x2 block by Bayer pattern (monochrome magnitudes; Average mode bins the whole region instead, below). local_x/local_y select the green nearest this pixel.
                    let local_x = sensor_x - block_x;
                    let local_y = sensor_y - block_y;
                    let (r_mono, g_mono, b_mono) = match ui.bayer_pattern {
                        0 => {
                            let g = if local_x + local_y < 2 { tr } else { bl };
                            (tl, g, br)
                        }
                        1 => {
                            let g = if local_x + local_y < 2 { tl } else { br };
                            (tr, g, bl)
                        }
                        2 => {
                            let g = if local_x + local_y < 2 { tl } else { br };
                            (bl, g, tr)
                        }
                        3 => {
                            let g = if local_x + local_y < 2 { tr } else { bl };
                            (br, g, tl)
                        }
                        _ => panic!("Unknown bayer pattern!"),
                    };

                    // Average mode: collapse the source CFA region under this screen pixel into one RGB pixel by binning every pixel of the region into its channel, then normalising each channel by how many samples it received. This one path serves BOTH layouts - standard Bayer (region = the 2x2 cell, colours 1xR 2xG 1xB) and quad-Bayer (region = the 4x4 Tetracell tile, colours 4xR 8xG 4xB). Binning every pixel (not point-sampling one representative) is what removes the quad grid and the standard 2x2 single-pick; summing both greens then dividing each channel by its own sample count keeps the colour balance correct for either layout. No divide in the loop: the per-channel normalisation is a single reciprocal multiply per channel. Diff/Motion keep the monochrome 2x2 pick below.
                    let (r, g, b) = if current_mode == RawMode::Average {
                        let last = ui.sensor_x_size * ui.sensor_y_size - 1;
                        // Region size: quad collapses a 4x4 tile, standard a single 2x2 cell. tx/ty is the region origin (tile-aligned for quad, cell-aligned for standard).
                        let region = if is_quad { 4 } else { 2 };
                        let mask = !(region - 1);
                        let tx = sensor_x & mask;
                        let ty = sensor_y & mask;
                        // Per-2x2-cell colour (0=R,1=G,2=B) of the active pattern, indexed row-major (idx = row*2 + col). For quad this is the colour of each 2x2 CLUSTER of the 4x4 tile; for standard it is the colour of each pixel of the 2x2 cell. cstep is how many source pixels one base cell spans (2 for quad clusters, 1 for standard pixels).
                        let cells = base_2x2(ui.bayer_pattern);
                        let cstep = region / 2; // 2 for quad, 1 for standard
                        // Same black-subtract / clip indicator as the Average arm above: controls visible -> white-clip zeros over-threshold samples AND black-clip via WRAPPING sub (crushed shadows render WHITE); controls hidden -> saturating sub.
                        let sub = |v: u16| -> usize {
                            if ui.controls_visible {
                                if v > ui_constants::CLIPPING_THRESHOLD {
                                    0
                                } else {
                                    v.wrapping_sub(ui.raw_black_level) as usize
                                }
                            } else {
                                v.saturating_sub(ui.raw_black_level) as usize
                            }
                        };
                        let (mut rsum, mut gsum, mut bsum) = (0usize, 0usize, 0usize);
                        for dy in 0..region {
                            for dx in 0..region {
                                // Which base 2x2 cell this source pixel falls in -> its CFA colour. (dx/cstep, dy/cstep) is 0..1 for both layouts.
                                let cell = (dy / cstep) * 2 + (dx / cstep);
                                // .min(last): clamps the index feeding raw_average[..] to the last valid element so a region origin at the sensor's right/bottom edge can't index out of bounds (panic / UB).
                                let idx = ((ty + dy) * ui.sensor_x_size + tx + dx).min(last);
                                let s = sub(raw_average[idx]);
                                match cells[cell] {
                                    0 => rsum += s,
                                    2 => bsum += s,
                                    _ => gsum += s,
                                }
                            }
                        }
                        // Normalise each channel to single-pixel magnitude (the same domain as the calibration overlay and `scale` below) by dividing by its own sample count. Counts: standard 1/2/1, quad 4/8/4. Reciprocals are multiplies - no divide in the loop. half = region*region/4 is the R/B count (a quarter of the region); green gets twice that.
                        let half = (region * region / 4) as f32;
                        (
                            rsum as f32 / half,
                            gsum as f32 / (half * 2.),
                            bsum as f32 / half,
                        )
                    } else {
                        (r_mono as f32, g_mono as f32, b_mono as f32)
                    };

                    // Composite the calibration overlay HERE, in RAW space, BEFORE scale/matrix/sqrt - the overlay holds raw-native values (chameleon builds it from injectmagic9, the same raw the RAW-injection path writes), so it must replace the raw sensor values and then flow through the EXACT SAME display pipeline as every other pixel (scale -> display matrix -> sqrt). Compositing it after the adjustments was the whole bug: it then had to fake-replicate scale/matrix and never matched. Overlay coords (min_x/min_y/ov_w/ov_h) are in the scan-input frame: full sensor for standard Bayer, half-res binned for quad (ui.rs bins by 2 before scan_target), so a full-res sensor coord maps in via >>1 for quad and unchanged for standard.
                    let (mut r, mut g, mut b) = (r, g, b);
                    if let Some((min_x, min_y, ov_w, ov_h, overlay)) = cal_overlay {
                        let ox = if is_quad { sensor_x >> 1 } else { sensor_x };
                        let oy = if is_quad { sensor_y >> 1 } else { sensor_y };
                        if ox >= *min_x && ox < min_x + ov_w && oy >= *min_y && oy < min_y + ov_h {
                            let ov_idx = ((oy - min_y) * ov_w + (ox - min_x)) * 4;
                            if ov_idx + 3 < overlay.len() {
                                let alpha = overlay[ov_idx + 3];
                                if alpha > 0.0 {
                                    // Overlay is already in black-subtracted raw counts (chameleon scaled reflectance by the scanned white-patch level minus black), the SAME domain as the scene r/g/b here. Composite raw-for-raw; the normal scale/matrix/sqrt below then renders it identically to the real target at the current exposure.
                                    let ia = 1.0 - alpha;
                                    r = overlay[ov_idx] * alpha + r * ia;
                                    g = overlay[ov_idx + 1] * alpha + g * ia;
                                    b = overlay[ov_idx + 2] * alpha + b * ia;
                                }
                            }
                        }
                    }

                    // Apply display scaling based on mode
                    let scale = match current_mode {
                        RawMode::Average => {
                            ui.display_gain as f32 * (65536. / (65536. - ui.raw_black_level as f32))
                        }
                        RawMode::Difference | RawMode::Motion => ui.display_gain as f32,
                    };

                    // Linear scale, then apply the camera->Rec.2020 display matrix in LINEAR space (mixes channels), then sqrt-encode for the BT.2020 surface. Average mode is colour (debayered) so colour-correct; diff/motion are monochrome magnitudes - leave them unmixed.
                    let (lr, lg, lb) = (r * scale, g * scale, b * scale);

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

    // Progress bar on the exposure-time slider. A flat line/band of the original slider thickness grows from the slider START toward a moving point ("fill"), and each scanline of the band stops at the LEFT/NEAR OUTLINE of the slider circle positioned at that fill point - so the leading end is a CONCAVE cup the slider dot nestles into (rows near the band centre reach furthest toward the dot; rows near the band edges stop shorter where the circle bulges into them). fill = exposure-completion (elapsed/total_ms, 0..1, resets each exposure) scaled to the exposure-time dot's slider position (ui.exposure_time_slider, which already has the sqrt log mapping baked in - we do NOT re-apply it). Colour sweeps red (fresh) -> yellow (half) -> green (complete) on the same completion fraction.
    if ui.slider_thickness > 0.0 && total_ms > 0 {
        // Fraction through the CURRENT exposure (0 at start, 1 at completion); resets each exposure. min(1.0) caps overrun.
        let completion = (elapsed as f32 / total_ms as f32).min(1.0);
        let fill = completion * ui.exposure_time_slider as f32;

        // Bar colour from completion: red fades out, green fades in -> yellow mid -> green at completion. .clamp guards float drift before the saturating u8 cast.
        let cr = (((1.0 - completion) * 2.0).clamp(0.0, 1.0) * 255.0) as u8;
        let cg = ((completion * 2.0).clamp(0.0, 1.0) * 255.0) as u8;
        let (cr, cg, cb) = (cr, cg, 0u8);

        let is_vertical = (ui.slider_start_x - ui.slider_end_x).abs() < 1.0;
        let stroke = ui.slider_thickness as i32; // band half-thickness (original line width, unchanged)
        let circle_r = ui.slider_thickness * 2.0; // slider dot radius (slider_thickness is stored as circle_radius/2)

        if is_vertical {
            // Band runs vertically (along Y); scanlines are columns offset in X from the centre line.
            let center_x = ui.slider_start_x as i32;
            let start_y = ui.slider_start_y;
            let circle_cy = start_y + (ui.slider_end_y - ui.slider_start_y) * fill; // dot centre at the fill point
            let grows_down = ui.slider_end_y >= ui.slider_start_y;
            for dx in -stroke..=stroke {
                let x = center_x + dx;
                if x < 0 || (x as usize) >= buffer.stride as usize {
                    continue;
                }
                // This scanline stops at the circle's near outline at its X offset: the circle reaches (circle_r^2 - dx^2) past its centre along Y. .max(0.0) guards float error before sqrt; columns outside the circle's X extent (|dx|>circle_r) just stop at the centre.
                let reach = (circle_r * circle_r - (dx * dx) as f32).max(0.0).sqrt();
                let edge = if grows_down { circle_cy - reach } else { circle_cy + reach };
                let (lo, hi) = if grows_down { (start_y, edge) } else { (edge, start_y) };
                let yi0 = lo.max(0.0) as usize;
                let yi1 = (hi.max(0.0) as usize).min(buffer.height as usize - 1);
                if yi0 > yi1 {
                    continue;
                }
                for y in yi0..=yi1 {
                    let idx = (y * buffer.stride as usize + x as usize) * 3;
                    if idx + 2 < pixels.len() {
                        pixels[idx] = cr;
                        pixels[idx + 1] = cg;
                        pixels[idx + 2] = cb;
                    }
                }
            }
        } else {
            // Band runs horizontally (along X); scanlines are rows offset in Y from the centre line.
            let center_y = ui.slider_start_y as i32;
            let start_x = ui.slider_start_x;
            let circle_cx = start_x + (ui.slider_end_x - ui.slider_start_x) * fill; // dot centre at the fill point
            let grows_right = ui.slider_end_x >= ui.slider_start_x;
            for dy in -stroke..=stroke {
                let y = center_y + dy;
                if y < 0 || (y as usize) >= buffer.height as usize {
                    continue;
                }
                // Stop this row at the circle's near outline at its Y offset. .max(0.0) guards float error before sqrt; rows outside the circle's Y extent stop at the centre.
                let reach = (circle_r * circle_r - (dy * dy) as f32).max(0.0).sqrt();
                let edge = if grows_right { circle_cx - reach } else { circle_cx + reach };
                let (lo, hi) = if grows_right { (start_x, edge) } else { (edge, start_x) };
                let xi0 = lo.max(0.0) as usize;
                let xi1 = (hi.max(0.0) as usize).min(buffer.stride as usize - 1);
                if xi0 > xi1 {
                    continue;
                }
                for x in xi0..=xi1 {
                    let idx = (y as usize * buffer.stride as usize + x) * 3;
                    if idx + 2 < pixels.len() {
                        pixels[idx] = cr;
                        pixels[idx + 1] = cg;
                        pixels[idx + 2] = cb;
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
        SAVE_FORMAT_JPEGXL => "JXL",
        SAVE_FORMAT_JPEG => "JPEG",
        SAVE_FORMAT_DNG => "DNG",
        SAVE_FORMAT_TIFF => "TIFF",
        _ => "JXL",
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
                // +32 blue is a debug tint to visualise the counter restore region; in production restore the original pixel untinted. (Wrapping add, like the rest of the codebase; the tint values never approach overflow anyway.)
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
