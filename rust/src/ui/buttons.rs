use crate::ui::ui::ui_constants;
use rand::Rng;

pub struct ButtonBuffers {
    pub skinny_normal: Vec<Vec<u8>>,
    pub skinny_pressed: Vec<Vec<u8>>,
    pub skinny_alpha: Vec<Vec<u8>>,
    pub fat_normal: Vec<Vec<u8>>,
    pub fat_pressed: Vec<Vec<u8>>,
    pub fat_alpha: Vec<Vec<u8>>,
    pub skinny_run: usize,
    pub skinny_rise: usize,
    pub fat_run: usize,
    pub fat_rise: usize,
}
pub struct CalibrationButtonBuffer {
    pub normal_buffer: Vec<u8>,  // RGBA
    pub pressed_buffer: Vec<u8>, // RGBA
    pub size: usize,             // Square buffer
}

impl ButtonBuffers {
    pub fn new_from_dimensions(
        screen_run: usize,
        screen_rise: usize,
        long_edge: usize,
        short_edge: usize,
        long_margin: f32,
        short_margin: f32,
    ) -> Self {
        let track_height_skinny = (long_edge as f32 - long_margin * 2.)
            * ui_constants::CONTROLS_HEIGHT_SKINNY
            / ui_constants::TOTAL_TRACKS as f32;
        let track_height_fat = (short_edge as f32 - short_margin * 2.)
            * ui_constants::CONTROLS_HEIGHT_FAT
            / ui_constants::TOTAL_TRACKS as f32;

        let skinny_width =
            ((short_edge as f32 - short_margin * 2.) * ui_constants::SKINNY_BUTTON_WIDTH) as usize;
        let skinny_height = track_height_skinny as usize;
        let fat_width =
            ((long_edge as f32 - long_margin * 2.) * ui_constants::FAT_BUTTON_WIDTH) as usize;
        let fat_height = track_height_fat as usize;
        let (skinny_run, skinny_rise, fat_run, fat_rise) = if screen_run < screen_rise {
            (skinny_width, skinny_height, fat_height, fat_width)
        } else {
            (skinny_height, skinny_width, fat_width, fat_height)
        };

        let mut buffers = Self {
            skinny_normal: Vec::with_capacity(4),
            skinny_pressed: Vec::with_capacity(4),
            skinny_alpha: Vec::with_capacity(4),
            fat_normal: Vec::with_capacity(4),
            fat_pressed: Vec::with_capacity(4),
            fat_alpha: Vec::with_capacity(4),
            skinny_run,
            skinny_rise,
            fat_run,
            fat_rise,
        };

        buffers.render_all_buttons();
        buffers
    }

    fn render_all_buttons(&mut self) {
        let button_colours = ui_constants::BUTTON_COLOURS;

        for button_idx in 0..4 {
            let base_colour = button_colours[button_idx];
            let pressed_colour = (
                base_colour.0 * 1.5,
                base_colour.1 * 1.5,
                base_colour.2 * 1.5,
            );

            // Render skinny buttons
            let mut normal_buffer = Vec::with_capacity(self.skinny_run * self.skinny_rise * 3);
            let mut pressed_buffer = Vec::with_capacity(self.skinny_run * self.skinny_rise * 3);
            let mut alpha_buffer = Vec::with_capacity(self.skinny_run * self.skinny_rise * 3);

            let min_dim = self.skinny_run.min(self.skinny_rise);

            for y in 0..self.skinny_rise {
                for x in 0..self.skinny_run {
                    let xf = (1.
                        - (x as f32).min(self.skinny_run as f32 - x as f32) * 2. / min_dim as f32)
                        .max(0.);
                    let yf = (1.
                        - (y as f32).min(self.skinny_rise as f32 - y as f32) * 2. / min_dim as f32)
                        .max(0.);
                    let vf = xf * xf * xf + yf * yf * yf;

                    // Red channel
                    let mut rf = vf;
                    rf = rf * rf * 1.25;
                    rf = rf * rf;
                    rf = rf * rf;
                    rf = rf * rf;
                    rf = 1. - rf;
                    rf = 1. - rf * (31. / 16.);
                    let raf = rf * rf;
                    rf = 1. - raf;
                    let r_normal = (rf * 256. * base_colour.0) as u8;
                    let r_pressed = (rf * 256. * pressed_colour.0) as u8;
                    let ra = (raf * 256.) as u8;

                    // Green channel
                    let mut gf = vf;
                    gf = gf * gf * 1.3;
                    gf = gf * gf;
                    gf = gf * gf;
                    gf = gf * gf;
                    gf = 1. - gf;
                    gf = 1. - gf * (31. / 16.);
                    let gaf = gf * gf;
                    gf = 1. - gaf;
                    let g_normal = (gf * 256. * base_colour.1) as u8;
                    let g_pressed = (gf * 256. * pressed_colour.1) as u8;
                    let ga = (gaf * 256.) as u8;

                    // Blue channel
                    let mut bf = vf;
                    bf = bf * bf * 1.4;
                    bf = bf * bf;
                    bf = bf * bf;
                    bf = bf * bf;
                    bf = 1. - bf;
                    bf = 1. - bf * (31. / 16.);
                    let baf = bf * bf;
                    bf = 1. - baf;
                    let b_normal = (bf * 256. * base_colour.2) as u8;
                    let b_pressed = (bf * 256. * pressed_colour.2) as u8;
                    let ba = (baf * 256.) as u8;

                    normal_buffer.push(r_normal);
                    normal_buffer.push(g_normal);
                    normal_buffer.push(b_normal);

                    pressed_buffer.push(r_pressed);
                    pressed_buffer.push(g_pressed);
                    pressed_buffer.push(b_pressed);

                    alpha_buffer.push(ra);
                    alpha_buffer.push(ga);
                    alpha_buffer.push(ba);
                }
            }

            self.skinny_normal.push(normal_buffer);
            self.skinny_pressed.push(pressed_buffer);
            self.skinny_alpha.push(alpha_buffer);

            // Render fat buttons
            let mut fat_normal_buffer = Vec::with_capacity(self.fat_run * self.fat_rise * 3);
            let mut fat_pressed_buffer = Vec::with_capacity(self.fat_run * self.fat_rise * 3);
            let mut fat_alpha_buffer = Vec::with_capacity(self.fat_run * self.fat_rise * 3);

            let fat_min_dim = self.fat_run.min(self.fat_rise);

            for y in 0..self.fat_rise {
                for x in 0..self.fat_run {
                    let xf = (1.
                        - (x as f32).min(self.fat_run as f32 - x as f32) * 2. / fat_min_dim as f32)
                        .max(0.);
                    let yf = (1.
                        - (y as f32).min(self.fat_rise as f32 - y as f32) * 2.
                            / fat_min_dim as f32)
                        .max(0.);
                    let vf = xf * xf * xf + yf * yf * yf;

                    // Red channel
                    let mut rf = vf;
                    rf = rf * rf * 1.25;
                    rf = rf * rf;
                    rf = rf * rf;
                    rf = rf * rf;
                    rf = 1. - rf;
                    rf = 1. - rf * (31. / 16.);
                    let raf = rf * rf;
                    rf = 1. - raf;
                    let r_normal = (rf * 256. * base_colour.0) as u8;
                    let r_pressed = (rf * 256. * pressed_colour.0) as u8;
                    let ra = (raf * 256.) as u8;

                    // Green channel
                    let mut gf = vf;
                    gf = gf * gf * 1.3;
                    gf = gf * gf;
                    gf = gf * gf;
                    gf = gf * gf;
                    gf = 1. - gf;
                    gf = 1. - gf * (31. / 16.);
                    let gaf = gf * gf;
                    gf = 1. - gaf;
                    let g_normal = (gf * 256. * base_colour.1) as u8;
                    let g_pressed = (gf * 256. * pressed_colour.1) as u8;
                    let ga = (gaf * 256.) as u8;

                    // Blue channel
                    let mut bf = vf;
                    bf = bf * bf * 1.4;
                    bf = bf * bf;
                    bf = bf * bf;
                    bf = bf * bf;
                    bf = 1. - bf;
                    bf = 1. - bf * (31. / 16.);
                    let baf = bf * bf;
                    bf = 1. - baf;
                    let b_normal = (bf * 256. * base_colour.2) as u8;
                    let b_pressed = (bf * 256. * pressed_colour.2) as u8;
                    let ba = (baf * 256.) as u8;

                    fat_normal_buffer.push(r_normal);
                    fat_normal_buffer.push(g_normal);
                    fat_normal_buffer.push(b_normal);

                    fat_pressed_buffer.push(r_pressed);
                    fat_pressed_buffer.push(g_pressed);
                    fat_pressed_buffer.push(b_pressed);

                    fat_alpha_buffer.push(ra);
                    fat_alpha_buffer.push(ga);
                    fat_alpha_buffer.push(ba);
                }
            }

            self.fat_normal.push(fat_normal_buffer);
            self.fat_pressed.push(fat_pressed_buffer);
            self.fat_alpha.push(fat_alpha_buffer);
        }
    }
}

pub fn composite_button(
    pixels: &mut [u8],
    x_center: usize,
    y_center: usize,
    button_buffers: &ButtonBuffers,
    button_index: usize,
    is_fat: bool,
    is_pressed: bool,
    buffer_stride: usize,
) {
    let (colour_buffer, alpha_buffer, width, height) = if is_fat {
        let colour = if is_pressed {
            &button_buffers.fat_pressed[button_index]
        } else {
            &button_buffers.fat_normal[button_index]
        };
        (
            colour,
            &button_buffers.fat_alpha[button_index],
            button_buffers.fat_run,
            button_buffers.fat_rise,
        )
    } else {
        let colour = if is_pressed {
            &button_buffers.skinny_pressed[button_index]
        } else {
            &button_buffers.skinny_normal[button_index]
        };
        (
            colour,
            &button_buffers.skinny_alpha[button_index],
            button_buffers.skinny_run,
            button_buffers.skinny_rise,
        )
    };

    // Draw button with alpha blending
    for dy in 0..height {
        for dx in 0..width {
            let dst_x = x_center - width / 2 + dx;
            let dst_y = y_center - height / 2 + dy;

            let src_idx = (dy * width + dx) * 3;
            let dst_idx = (dst_y * buffer_stride + dst_x) * 3;

            let src_r = colour_buffer[src_idx] as u16;
            let src_g = colour_buffer[src_idx + 1] as u16;
            let src_b = colour_buffer[src_idx + 2] as u16;

            let alpha_r = alpha_buffer[src_idx] as u16;
            let alpha_g = alpha_buffer[src_idx + 1] as u16;
            let alpha_b = alpha_buffer[src_idx + 2] as u16;

            if crate::DEBUG {
                pixels[dst_idx] =
                    (((pixels[dst_idx] as u16 * (alpha_r + 1)) >> 8) + src_r).min(255) as u8 + 16;
            } else {
                pixels[dst_idx] =
                    (((pixels[dst_idx] as u16 * (alpha_r + 1)) >> 8) + src_r).min(255) as u8;
            }
            pixels[dst_idx + 1] =
                (((pixels[dst_idx + 1] as u16 * (alpha_g + 1)) >> 8) + src_g).min(255) as u8;
            pixels[dst_idx + 2] =
                (((pixels[dst_idx + 2] as u16 * (alpha_b + 1)) >> 8) + src_b).min(255) as u8;
        }
    }
}

impl CalibrationButtonBuffer {
    pub fn new(size: usize) -> Self {
        let mut buffer = Self {
            pressed_buffer: Vec::with_capacity(size * size * 4),
            normal_buffer: Vec::with_capacity(size * size * 4),
            size,
        };

        let x_patches = 7;
        let y_patches = 6;
        let mut random_patch_colours: Vec<(f32, f32, f32)> =
            Vec::with_capacity(x_patches * y_patches);
        let mut rng = rand::rng();

        // Generate random colours for each patch
        for _ in 0..(x_patches * y_patches) {
            random_patch_colours.push((rng.random(), rng.random(), rng.random()))
        }

        for y in 0..size {
            for x in 0..size {
                let mut xf = x as f32 / size as f32;
                xf = xf * 2. - 1.;
                let mut yf = y as f32 / size as f32;
                yf = yf * 2. - 1.;
                let mut xsf = xf;
                xsf = xsf * 4.;
                let mut ysf = yf;
                ysf = ysf * 4.;
                let x_patch = (xsf + 3.5).floor() as isize as usize;
                let y_patch = (ysf + 3.).floor() as isize as usize;
                let mut bullseye = false;
                let patch_shading = if xsf.abs() < 3.5 && ysf.abs() < 3. {
                    if xsf.abs() > 2.5 && ysf.abs() > 2. {
                        xsf = 1. - ((xsf - xsf.floor()) * 2. - 1.).abs();
                        ysf = ((ysf - ysf.floor()) * 2. - 1.).abs();
                        let sf =
                            (((xsf * xsf + ysf * ysf).sqrt().ln().min(0.) * 13.).cos() - 1.) / -2.;
                        bullseye = true;
                        sf * 0.75
                    } else {
                        xsf = 1. - ((xsf - xsf.floor()) * 2. - 1.).abs();
                        ysf = ((ysf - ysf.floor()) * 2. - 1.).abs();
                        xsf *= xsf;
                        ysf *= ysf;
                        xsf *= xsf;
                        ysf *= ysf;
                        let mut sf = xsf * xsf + ysf * ysf;
                        sf = sf * 3.;
                        sf = (1. - sf * sf).max(0.);
                        sf
                    }
                } else {
                    0.
                };

                xf *= xf;
                yf *= yf * 4. / 3.; // aspect
                xf *= xf;
                yf *= yf;
                xf *= xf;
                yf *= yf;
                xf *= xf;
                yf *= yf;

                let mut value_f = xf + yf;
                value_f *= value_f * 4.;
                value_f *= value_f;

                let alpha = (value_f - 1.).min(1.).max(0.);
                value_f = 1. - value_f;

                value_f = 1. - value_f * value_f + patch_shading;
                let colour_idx = y_patch * x_patches + x_patch;
                let (r, g, b) = if colour_idx < random_patch_colours.len() && x_patch < x_patches {
                    let colour = random_patch_colours[colour_idx];
                    if bullseye {
                        (value_f, value_f, value_f)
                    } else {
                        (
                            value_f * (colour.0 * 0.25 + 0.5),
                            value_f * (colour.1 * 0.25 + 0.5),
                            value_f * (colour.2 * 0.25 + 0.5),
                        )
                    }
                } else if bullseye {
                    (0., 0., 0.)
                } else {
                    (value_f * 0.5, value_f * 0.5, value_f * 0.5)
                };

                buffer.normal_buffer.push((r * 256.) as u8);
                buffer.normal_buffer.push((g * 256.) as u8);
                buffer.normal_buffer.push((b * 256.) as u8);
                buffer.normal_buffer.push((alpha * 256.) as u8);
                buffer.pressed_buffer.push((r * 384.) as u8);
                buffer.pressed_buffer.push((g * 384.) as u8);
                buffer.pressed_buffer.push((b * 384.) as u8);
                buffer.pressed_buffer.push((alpha * 384.) as u8);
            }
        }
        buffer
    }
}

pub fn composite_chameleon_button(
    pixels: &mut [u8],
    x_center: usize,
    y_center: usize,
    rotation: u16,
    button_buffer: &CalibrationButtonBuffer,
    is_pressed: bool,
    is_calibrating: bool,
    buffer_stride: usize,
) {
    let size = button_buffer.size;

    // Determine which buffer to use:
    // - If calibrating: blink between pressed/normal based on time (~2Hz)
    // - If pressed: use pressed buffer
    // - Otherwise: use normal buffer
    let use_pressed = if is_calibrating {
        // Blink at ~2Hz using system time
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        (millis / 250) % 2 == 0
    } else {
        is_pressed
    };

    let colour_buffer = if use_pressed {
        &button_buffer.pressed_buffer
    } else {
        &button_buffer.normal_buffer
    };

    let (x_start, y_start) = match rotation {
        0 => (x_center, y_center),
        90 => (x_center, y_center - size),
        180 => (x_center - size, y_center - size),
        270 => (x_center - size, y_center),
        _ => panic!("Invalid rotation"),
    };

    // Draw button with alpha blending from center, handling all four rotations
    for dy in 0..size {
        let dst_y = y_start + dy;
        for dx in 0..size {
            let dst_x = x_start + dx;
            // Apply rotation-specific source and destination coordinate mapping
            let (src_x, src_y) = match rotation {
                0 => (dx, dy),
                90 => (size - 1 - dy, dx),
                180 => (size - 1 - dx, size - 1 - dy),
                270 => (dy, size - 1 - dx),
                _ => panic!("Invalid rotation"),
            };

            let src_idx = (src_y * size + src_x) * 4; // RGBA = 4 bytes
            let dst_idx = (dst_y * buffer_stride + dst_x) * 3; // RGB = 3 bytes

            let src_r = colour_buffer[src_idx] as u16;
            let src_g = colour_buffer[src_idx + 1] as u16;
            let src_b = colour_buffer[src_idx + 2] as u16;
            let alpha = colour_buffer[src_idx + 3] as u16;

            pixels[dst_idx] =
                (((pixels[dst_idx] as u16 * (alpha + 1)) >> 8) + src_r).min(255) as u8;
            pixels[dst_idx + 1] =
                (((pixels[dst_idx] as u16 * (alpha + 1)) >> 8) + src_g).min(255) as u8;
            pixels[dst_idx + 2] =
                (((pixels[dst_idx + 2] as u16 * (alpha + 1)) >> 8) + src_b).min(255) as u8;
        }
    }
}

/// Draw the cropped target-scan (RGB, `src_w` x `src_h`) into the calibration-button slot,
/// scaled to `size` x `size`, opaque. Replaces the chameleon button after a successful
/// calibration as a "calibrated" indicator. Uses the same rotation/anchor mapping as
/// `composite_chameleon_button`. `pressed` dims it slightly for tap feedback.
pub fn composite_calibration_scan(
    pixels: &mut [u8],
    x_center: usize,
    y_center: usize,
    rotation: u16,
    button_buffer: &CalibrationButtonBuffer,
    src_w: u32,
    src_h: u32,
    src: &[u8],
    pressed: bool,
    buffer_stride: usize,
) {
    let size = button_buffer.size;
    if size == 0 || src_w == 0 || src_h == 0 || src.len() < (src_w * src_h * 3) as usize {
        return;
    }
    // Honour the chameleon button's OWN rounded-corner alpha (the 4th RGBA channel, already computed at this exact size) as the crop mask - so the captured target is cropped to the identical rounded-corner shape as the live preview, no re-derived corner math. alpha = background-keep factor (corners high -> keep background, interior 0 -> fully show scan).
    let alpha_src = &button_buffer.normal_buffer;
    let (x_start, y_start) = match rotation {
        0 => (x_center, y_center),
        90 => (x_center, y_center - size),
        180 => (x_center - size, y_center - size),
        270 => (x_center - size, y_center),
        _ => return,
    };
    let dim = if pressed { 200u16 } else { 256u16 }; // slight darken when pressed

    // The scan is 7:6 (wider than tall). Fit it into the square button preserving aspect
    // (letterbox), centred. fit = scaled size in button px; off = letterbox offset.
    let (fit_w, fit_h, off_x, off_y) = if src_w * size as u32 >= src_h * size as u32 {
        // width-limited: full button width, shorter height
        let fh = (size as u32 * src_h / src_w) as usize;
        (size, fh, 0usize, (size - fh) / 2)
    } else {
        let fw = (size as u32 * src_w / src_h) as usize;
        (fw, size, (size - fw) / 2, 0usize)
    };

    for dy in 0..size {
        let dst_y = y_start + dy;
        for dx in 0..size {
            let dst_x = x_start + dx;
            // Button-local coords in the un-rotated frame (same rotation mapping composite_chameleon_button uses).
            let (bx, by) = match rotation {
                0 => (dx, dy),
                90 => (size - 1 - dy, dx),
                180 => (size - 1 - dx, size - 1 - dy),
                270 => (dy, size - 1 - dx),
                _ => (dx, dy),
            };
            let dst_idx = (dst_y * buffer_stride + dst_x) * 3;
            // The button's stored rounded-corner alpha at this same button-local pixel (RGBA, 4th byte). It's the background-keep factor: corners high (keep background) -> rounded transparent corners; interior ~0 (show scan). Identical mask to the live preview, by construction.
            let alpha = alpha_src[(by * size + bx) * 4 + 3] as u16;

            // Scan colour (black in the aspect-fit letterbox; the corner alpha handles the rounding).
            let (sr, sg, sb) = if bx < off_x || bx >= off_x + fit_w || by < off_y || by >= off_y + fit_h {
                (0u16, 0u16, 0u16)
            } else {
                let sx = ((bx - off_x) as u32 * src_w / fit_w as u32).min(src_w - 1);
                let sy = ((by - off_y) as u32 * src_h / fit_h as u32).min(src_h - 1);
                let src_idx = ((sy * src_w + sx) * 3) as usize;
                (
                    (src[src_idx] as u16 * dim) >> 8,
                    (src[src_idx + 1] as u16 * dim) >> 8,
                    (src[src_idx + 2] as u16 * dim) >> 8,
                )
            };
            // Proper alpha-over: out = bg*alpha + scan*(255-alpha), all /255. The button's own blend is bg*alpha + src because ITS colour buffer is PREMULTIPLIED by coverage at generation; the scan is full-strength RGB (not premultiplied), so it must be attenuated by the foreground factor (255-alpha) - otherwise the anti-aliased corner ramp adds the scan at full strength over bg*alpha and the edges over-brighten (the aliasing artefact). fg = 255 - alpha.
            let fg = 255 - alpha;
            pixels[dst_idx] = ((pixels[dst_idx] as u16 * alpha + sr * fg) / 255).min(255) as u8;
            pixels[dst_idx + 1] = ((pixels[dst_idx + 1] as u16 * alpha + sg * fg) / 255).min(255) as u8;
            pixels[dst_idx + 2] = ((pixels[dst_idx + 2] as u16 * alpha + sb * fg) / 255).min(255) as u8;
        }
    }
}
