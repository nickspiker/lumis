use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};

pub struct TextRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
}

impl TextRenderer {
    pub fn new() -> Self {
        let mut font_system = FontSystem::new();

        let db = font_system.db_mut();

        db.load_font_data(include_bytes!("../../assets/Oxanium/Oxanium-ExtraLight.ttf").to_vec());
        db.load_font_data(include_bytes!("../../assets/Oxanium/Oxanium-Light.ttf").to_vec());
        db.load_font_data(include_bytes!("../../assets/Oxanium/Oxanium-Regular.ttf").to_vec());
        db.load_font_data(include_bytes!("../../assets/Oxanium/Oxanium-Medium.ttf").to_vec());
        db.load_font_data(include_bytes!("../../assets/Oxanium/Oxanium-SemiBold.ttf").to_vec());
        db.load_font_data(include_bytes!("../../assets/Oxanium/Oxanium-Bold.ttf").to_vec());
        db.load_font_data(include_bytes!("../../assets/Oxanium/Oxanium-ExtraBold.ttf").to_vec());

        Self {
            font_system,
            swash_cache: SwashCache::new(),
        }
    }

    /// Convert text for rendering, tracking μ positions
    fn preprocess_text(text: &str) -> (String, Vec<usize>) {
        let mut mu_positions = Vec::new();
        let mut glyph_index = 0;

        let processed = text
            .chars()
            .map(|ch| {
                let result = match ch {
                    'μ' => {
                        mu_positions.push(glyph_index);
                        'h'
                    }
                    _ => ch,
                };
                glyph_index += 1;
                result
            })
            .collect();

        (processed, mu_positions)
    }

    pub fn draw_text_center(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        weight: u16,
        r: u8,
        g: u8,
        b: u8,
        rotation: u16,
    ) -> f32 {
        let (processed_text, mu_positions) = Self::preprocess_text(text);

        let attrs = Attrs::new()
            .family(Family::Name("Oxanium"))
            .weight(Weight(weight))
            .color(Color::rgb(r, g, b));

        let metrics = Metrics::relative(size, 1.2);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);

        buffer.set_size(&mut self.font_system, None, None);
        buffer.set_text(
            &mut self.font_system,
            &processed_text,
            &attrs,
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        if let Some(run) = buffer.layout_runs().next() {
            // Calculate text width
            let mut min_x = f32::MAX;
            let mut max_x = f32::MIN;

            for glyph in run.glyphs {
                min_x = min_x.min(glyph.x);
                max_x = max_x.max(glyph.x + glyph.w);
            }

            let text_width = max_x - min_x;
            let text_height = run.line_height;

            self.render_buffer_with_mu(
                &mut buffer,
                pixels,
                width,
                height,
                x,
                y,
                text_width,
                text_height,
                r,
                g,
                b,
                &mu_positions,
                rotation,
                0, // center alignment
            );

            text_width
        } else {
            0.0
        }
    }

    pub fn draw_text_left(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        weight: u16,
        r: u8,
        g: u8,
        b: u8,
        rotation: u16,
    ) -> f32 {
        let (processed_text, mu_positions) = Self::preprocess_text(text);

        let attrs = Attrs::new()
            .family(Family::Name("Oxanium"))
            .weight(Weight(weight))
            .color(Color::rgb(r, g, b));

        let metrics = Metrics::relative(size, 1.2);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);

        buffer.set_size(&mut self.font_system, None, None);
        buffer.set_text(
            &mut self.font_system,
            &processed_text,
            &attrs,
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        if let Some(run) = buffer.layout_runs().next() {
            let mut text_width = 0.0f32;
            for glyph in run.glyphs {
                text_width = text_width.max(glyph.x + glyph.w);
            }
            let text_height = run.line_height;

            self.render_buffer_with_mu(
                &mut buffer,
                pixels,
                width,
                height,
                x,
                y,
                text_width,
                text_height,
                r,
                g,
                b,
                &mu_positions,
                rotation,
                1, // left alignment
            );

            text_width
        } else {
            0.0
        }
    }

    pub fn draw_text_right(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        weight: u16,
        r: u8,
        g: u8,
        b: u8,
        rotation: u16,
    ) -> f32 {
        let (processed_text, mu_positions) = Self::preprocess_text(text);

        let attrs = Attrs::new()
            .family(Family::Name("Oxanium"))
            .weight(Weight(weight))
            .color(Color::rgb(r, g, b));

        let metrics = Metrics::relative(size, 1.2);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);

        buffer.set_size(&mut self.font_system, None, None);
        buffer.set_text(
            &mut self.font_system,
            &processed_text,
            &attrs,
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        if let Some(run) = buffer.layout_runs().next() {
            let mut text_width = 0.0f32;
            for glyph in run.glyphs {
                text_width = text_width.max(glyph.x + glyph.w);
            }
            let text_height = run.line_height;

            self.render_buffer_with_mu(
                &mut buffer,
                pixels,
                width,
                height,
                x,
                y,
                text_width,
                text_height,
                r,
                g,
                b,
                &mu_positions,
                rotation,
                2, // right alignment
            );

            text_width
        } else {
            0.0
        }
    }

    /// Render buffer to pixel array with μ support
    fn render_buffer_with_mu(
        &mut self,
        buffer: &mut Buffer,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        anchor_x: f32,
        anchor_y: f32,
        text_width: f32,
        text_height: f32,
        r: u8,
        g: u8,
        b: u8,
        mu_positions: &[usize],
        rotation: u16,
        alignment: u8, // 0=center, 1=left, 2=right
    ) {
        // Calculate the offset based on alignment
        let (offset_x, offset_y) = match alignment {
            0 => (anchor_x - text_width / 2.0, anchor_y - text_height / 2.0), // center
            1 => (anchor_x, anchor_y - text_height / 2.0),                    // left
            2 => (anchor_x - text_width, anchor_y - text_height / 2.0),       // right
            _ => (anchor_x, anchor_y),
        };

        for run in buffer.layout_runs() {
            let baseline_offset = run.line_y;
            let mut glyph_index = 0;

            for glyph in run.glyphs {
                let physical_glyph = glyph.physical((offset_x, offset_y), 1.0);

                if let Some(image) = self
                    .swash_cache
                    .get_image(&mut self.font_system, physical_glyph.cache_key)
                {
                    let glyph_x = physical_glyph.x + image.placement.left;
                    let glyph_y = physical_glyph.y + baseline_offset as i32 - image.placement.top;

                    // Check if this glyph should be flipped (was originally μ)
                    let should_flip = mu_positions.contains(&glyph_index);

                    // Draw the glyph bitmap
                    for (i, &alpha) in image.data.iter().enumerate() {
                        if alpha > 0 {
                            let cy = i / image.placement.width as usize;
                            let cx = i % image.placement.width as usize;

                            // Apply μ flipping
                            let py_base = if should_flip {
                                let flipped_y = (image.placement.height as usize - 1 - cy) as i32;
                                let adjustment = (image.placement.height as f32 * 0.3) as i32;
                                glyph_y + flipped_y + adjustment
                            } else {
                                glyph_y + cy as i32
                            };

                            let px_base = glyph_x + cx as i32;

                            // Calculate position relative to anchor point
                            let rel_x = px_base as f32 - anchor_x;
                            let rel_y = py_base as f32 - anchor_y;

                            // Rotate around the anchor point
                            let (rot_x, rot_y) = match rotation {
                                90 => (rel_y, -rel_x),
                                180 => (-rel_x, -rel_y),
                                270 => (-rel_y, rel_x),
                                _ => (rel_x, rel_y),
                            };

                            // Convert back to absolute coordinates
                            let final_x = (anchor_x + rot_x) as i32;
                            let final_y = (anchor_y + rot_y) as i32;

                            // Check bounds and render
                            if final_x >= 0
                                && final_y >= 0
                                && final_x < width as i32
                                && final_y < height as i32
                            {
                                let offset =
                                    (final_y as usize * width as usize + final_x as usize) * 3;

                                let alpha = alpha as u16;
                                let inv_alpha = 256 - alpha;

                                pixels[offset] = ((pixels[offset] as u16 * inv_alpha
                                    + r as u16 * alpha)
                                    >> 8) as u8;
                                pixels[offset + 1] =
                                    ((pixels[offset + 1] as u16 * inv_alpha + g as u16 * alpha)
                                        >> 8) as u8;
                                pixels[offset + 2] =
                                    ((pixels[offset + 2] as u16 * inv_alpha + b as u16 * alpha)
                                        >> 8) as u8;
                            }
                        }
                    }
                }

                glyph_index += 1;
            }
        }
    }
}
