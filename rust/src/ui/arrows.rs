use crate::ui::ui::ui_constants;

pub struct ArrowBuffers {
    pub skinny_normal: Vec<u8>,
    pub skinny_pressed: Vec<u8>,
    pub skinny_alpha: Vec<u8>,
    pub fat_normal: Vec<u8>,
    pub fat_pressed: Vec<u8>,
    pub fat_alpha: Vec<u8>,
    pub skinny_size: usize,
    pub fat_size: usize,
}

impl ArrowBuffers {
    pub fn new_from_dimensions(
        screen_run: usize,
        screen_rise: usize,
        long_edge: usize,
        short_edge: usize,
        long_margin: f32,
        short_margin: f32,
    ) -> Self {
        let skinny_size = ((long_edge as f32 - long_margin * 2.)
            * ui_constants::CONTROLS_HEIGHT_SKINNY
            / ui_constants::TOTAL_TRACKS as f32
            * ui_constants::ARROW_SIZE) as usize;
        let fat_size = ((short_edge as f32 - short_margin * 2.) * ui_constants::CONTROLS_HEIGHT_FAT
            / ui_constants::TOTAL_TRACKS as f32
            * ui_constants::ARROW_SIZE) as usize;

        let screen_native_fat = screen_rise > screen_run;

        let skinny_pixel_count = skinny_size * skinny_size * 3;
        let fat_pixel_count = fat_size * fat_size * 3;

        let mut buffers = Self {
            skinny_normal: Vec::with_capacity(skinny_pixel_count),
            skinny_pressed: Vec::with_capacity(skinny_pixel_count),
            skinny_alpha: Vec::with_capacity(skinny_pixel_count),
            fat_normal: Vec::with_capacity(fat_pixel_count),
            fat_pressed: Vec::with_capacity(fat_pixel_count),
            fat_alpha: Vec::with_capacity(fat_pixel_count),
            skinny_size,
            fat_size,
        };

        buffers.render_all_arrows(skinny_size, fat_size, screen_native_fat);
        buffers
    }

    fn render_all_arrows(&mut self, skinny_size: usize, fat_size: usize, _screen_native_fat: bool) {
        for y in 0..skinny_size {
            for x in 0..skinny_size {
                let mut xf = x as f32 / skinny_size as f32;
                let mut yf = y as f32 / skinny_size as f32;
                xf = xf * 2. - 1.;
                yf = 1. - yf * 2.;
                let (tx, ty) = if true { (xf, yf) } else { (-yf, xf) };

                let (r, g, b) = calculate_arrow_normal_colour(tx, ty);
                self.skinny_normal.push((r * 256.) as u8);
                self.skinny_normal.push((g * 256.) as u8);
                self.skinny_normal.push((b * 256.) as u8);

                let (r, g, b) = calculate_arrow_pressed_colour(tx, ty);
                self.skinny_pressed.push((r * 256.) as u8);
                self.skinny_pressed.push((g * 256.) as u8);
                self.skinny_pressed.push((b * 256.) as u8);

                let (r, g, b) = calculate_arrow_alpha(tx, ty);
                self.skinny_alpha.push((r * 256.) as u8);
                self.skinny_alpha.push((g * 256.) as u8);
                self.skinny_alpha.push((b * 256.) as u8);
            }
        }
        for y in 0..fat_size {
            for x in 0..fat_size {
                let mut xf = x as f32 / fat_size as f32;
                let mut yf = y as f32 / fat_size as f32;
                xf = xf * 2. - 1.;
                yf = 1. - yf * 2.;
                let (tx, ty) = if false { (xf, yf) } else { (-yf, xf) };

                let (r, g, b) = calculate_arrow_normal_colour(tx, ty);
                self.fat_normal.push((r * 256.) as u8);
                self.fat_normal.push((g * 256.) as u8);
                self.fat_normal.push((b * 256.) as u8);

                let (r, g, b) = calculate_arrow_pressed_colour(tx, ty);
                self.fat_pressed.push((r * 256.) as u8);
                self.fat_pressed.push((g * 256.) as u8);
                self.fat_pressed.push((b * 256.) as u8);

                let (r, g, b) = calculate_arrow_alpha(tx, ty);
                self.fat_alpha.push((r * 256.) as u8);
                self.fat_alpha.push((g * 256.) as u8);
                self.fat_alpha.push((b * 256.) as u8);
            }
        }
    }
}

fn calculate_arrow_normal_colour(x: f32, y: f32) -> (f32, f32, f32) {
    let x = (0.25 - x) / 4.;
    let y = y / 4.;
    let r_radius = 16.8;
    let g_radius = 17.1;
    let b_radius = 17.25;
    let r_taper = 9.;
    let g_taper = 8.;
    let b_taper = 15.;
    let r_scale = 1.;
    let g_scale = 0.8;
    let b_scale = 0.6;
    (
        (1. - (((r_radius / 32.
            - y * y
            - ((x + (y * y * x + y * y + 1. / 256.).sqrt() - 0.25).powi(2)
                + (y * ((x - 1.5) * (x - 1.5) + 1.)).powi(24))
            .powf(1. / 8.))
        .max(0.)
        .sqrt()
            * r_taper)
            .tanh()
            * 1.75
            - 1.)
            .powi(2))
            * r_scale,
        (1. - (((g_radius / 32.
            - y * y
            - ((x + (y * y * x + y * y + 1. / 256.).sqrt() - 0.25).powi(2)
                + (y * ((x - 1.5) * (x - 1.5) + 1.)).powi(24))
            .powf(1. / 8.))
        .max(0.)
        .sqrt()
            * g_taper)
            .tanh()
            * 1.75
            - 1.)
            .powi(2))
            * g_scale,
        (1. - (((b_radius / 32.
            - y * y
            - ((x + (y * y * x + y * y + 1. / 256.).sqrt() - 0.25).powi(2)
                + (y * ((x - 1.5) * (x - 1.5) + 1.)).powi(24))
            .powf(1. / 8.))
        .max(0.)
        .sqrt()
            * b_taper)
            .tanh()
            * 1.75
            - 1.)
            .powi(2))
            * b_scale,
    )
}
fn calculate_arrow_pressed_colour(x: f32, y: f32) -> (f32, f32, f32) {
    let x = (0.25 - x) / 4.;
    let y = y / 4.;
    let r_radius = 16.8;
    let g_radius = 17.1;
    let b_radius = 17.25;
    let r_taper = 16.;
    let g_taper = 7.;
    let b_taper = 22.;
    let r_scale = 0.7;
    let g_scale = 1.;
    let b_scale = 0.7;
    (
        (1. - (((r_radius / 32.
            - y * y
            - ((x + (y * y * x + y * y + 1. / 256.).sqrt() - 0.25).powi(2)
                + (y * ((x - 1.5) * (x - 1.5) + 1.)).powi(24))
            .powf(1. / 8.))
        .max(0.)
        .sqrt()
            * r_taper)
            .tanh()
            * 1.5
            - 1.)
            .powi(2))
            * r_scale,
        (1. - (((g_radius / 32.
            - y * y
            - ((x + (y * y * x + y * y + 1. / 256.).sqrt() - 0.25).powi(2)
                + (y * ((x - 1.5) * (x - 1.5) + 1.)).powi(24))
            .powf(1. / 8.))
        .max(0.)
        .sqrt()
            * g_taper)
            .tanh()
            * 1.25
            - 1.)
            .powi(2))
            * g_scale,
        (1. - (((b_radius / 32.
            - y * y
            - ((x + (y * y * x + y * y + 1. / 256.).sqrt() - 0.25).powi(2)
                + (y * ((x - 1.5) * (x - 1.5) + 1.)).powi(24))
            .powf(1. / 8.))
        .max(0.)
        .sqrt()
            * b_taper)
            .tanh()
            * 1.5
            - 1.)
            .powi(2))
            * b_scale,
    )
}
fn calculate_arrow_alpha(x: f32, y: f32) -> (f32, f32, f32) {
    let x = (0.25 - x) / 4.;
    let y = y / 4.;
    let r_radius = 16.8;
    let g_radius = 17.1;
    let b_radius = 17.25;
    (
        1. - (((r_radius / 32.
            - y * y
            - ((x + (y * y * x + y * y + 1. / 256.).sqrt() - 0.25).powi(2)
                + (y * ((x - 1.5) * (x - 1.5) + 1.)).powi(24))
            .powf(1. / 8.))
        .max(0.)
        .sqrt()
            * 16.)
            .tanh()),
        1. - (((g_radius / 32.
            - y * y
            - ((x + (y * y * x + y * y + 1. / 256.).sqrt() - 0.25).powi(2)
                + (y * ((x - 1.5) * (x - 1.5) + 1.)).powi(24))
            .powf(1. / 8.))
        .max(0.)
        .sqrt()
            * 16.)
            .tanh()),
        1. - (((b_radius / 32.
            - y * y
            - ((x + (y * y * x + y * y + 1. / 256.).sqrt() - 0.25).powi(2)
                + (y * ((x - 1.5) * (x - 1.5) + 1.)).powi(24))
            .powf(1. / 8.))
        .max(0.)
        .sqrt()
            * 16.)
            .tanh()),
    )
}

pub fn composite_arrow(
    pixels: &mut [u8],
    center_x: usize,
    center_y: usize,
    arrow_buffers: &ArrowBuffers,
    is_fat: bool,
    is_pressed: bool,
    needs_flipped: bool,
    buffer_stride: usize,
) {
    let (colour_buffer, alpha_buffer, size) = if is_fat {
        let colour = if is_pressed {
            &arrow_buffers.fat_pressed
        } else {
            &arrow_buffers.fat_normal
        };
        (colour, &arrow_buffers.fat_alpha, arrow_buffers.fat_size)
    } else {
        let colour = if is_pressed {
            &arrow_buffers.skinny_pressed
        } else {
            &arrow_buffers.skinny_normal
        };
        (
            colour,
            &arrow_buffers.skinny_alpha,
            arrow_buffers.skinny_size,
        )
    };

    // Calculate top-left corner from center coordinates
    let half_size = size / 2;
    let x = center_x - half_size;
    let y = center_y - half_size;
    if needs_flipped {
        for dy in 0..size {
            for dx in 0..size {
                let dst_x = x + dx;
                let dst_y = y + dy;
                let src_x = (size - 1) - dx;
                let src_y = (size - 1) - dy;

                let src_idx = (src_y * size + src_x) * 3;
                let dst_idx = (dst_y * buffer_stride + dst_x) * 3;

                let src_r = colour_buffer[src_idx] as u16;
                let src_g = colour_buffer[src_idx + 1] as u16;
                let src_b = colour_buffer[src_idx + 2] as u16;
                let alpha_r = alpha_buffer[src_idx] as u16 + 1;
                let alpha_g = alpha_buffer[src_idx + 1] as u16 + 1;
                let alpha_b = alpha_buffer[src_idx + 2] as u16 + 1;

                pixels[dst_idx] =
                    (((pixels[dst_idx] as u16 * alpha_r) >> 8) + src_r).min(255) as u8;
                if crate::DEBUG {
                    pixels[dst_idx + 1] =
                        (((pixels[dst_idx + 1] as u16 * alpha_g) >> 8) + src_g).min(255) as u8 + 16;
                } else {
                    pixels[dst_idx + 1] =
                        (((pixels[dst_idx + 1] as u16 * alpha_g) >> 8) + src_g).min(255) as u8;
                }
                pixels[dst_idx + 2] =
                    (((pixels[dst_idx + 2] as u16 * alpha_b) >> 8) + src_b).min(255) as u8;
            }
        }
    } else {
        for dy in 0..size {
            for dx in 0..size {
                let dst_x = x + dx;
                let dst_y = y + dy;
                let src_x = dx;
                let src_y = dy;

                let src_idx = (src_y * size + src_x) * 3;
                let dst_idx = (dst_y * buffer_stride + dst_x) * 3;

                let src_r = colour_buffer[src_idx] as u16;
                let src_g = colour_buffer[src_idx + 1] as u16;
                let src_b = colour_buffer[src_idx + 2] as u16;
                let alpha_r = alpha_buffer[src_idx] as u16 + 1;
                let alpha_g = alpha_buffer[src_idx + 1] as u16 + 1;
                let alpha_b = alpha_buffer[src_idx + 2] as u16 + 1;

                if crate::DEBUG {
                    pixels[dst_idx] =
                        (((pixels[dst_idx] as u16 * alpha_r) >> 8) + src_r).min(255) as u8 + 16;
                } else {
                    pixels[dst_idx] =
                        (((pixels[dst_idx] as u16 * alpha_r) >> 8) + src_r).min(255) as u8;
                }
                pixels[dst_idx + 1] =
                    (((pixels[dst_idx + 1] as u16 * alpha_g) >> 8) + src_g).min(255) as u8;
                pixels[dst_idx + 2] =
                    (((pixels[dst_idx + 2] as u16 * alpha_b) >> 8) + src_b).min(255) as u8;
            }
        }
    }
}
