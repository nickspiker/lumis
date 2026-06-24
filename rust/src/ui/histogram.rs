use crate::ui::text::TextRenderer;

pub fn draw_histogram_overlay(
    raw_data: &[u16],
    raw_width: usize,
    raw_height: usize,
    black_level: u16,
    bayer_pattern: [u8; 4],
    pixels: &mut [u8],
    screen_width: usize,
    screen_height: usize,
    controls_visible: bool,
    rotation: u16,
    text_renderer: &mut TextRenderer,
) {
    let channels = 3;
    let fill = if controls_visible { 16 } else { 32 };
    let highlight = 192;
    let oversample = 32;

    // Pre-calculate rotation-specific dimensions and transformations
    let (effective_width, effective_height) = match rotation {
        90 | 270 => (screen_height, screen_width),
        _ => (screen_width, screen_height),
    };
    let margin = effective_width.min(effective_height) / 8; // Margin in effective coordinates

    // Determine if we're in skinny mode and which axis is constrained
    let is_skinny = effective_width < effective_height;
    let skip_x_labels = is_skinny; // Skip x-axis labels when width is constrained
    let skip_y_labels = !is_skinny; // Skip y-axis labels when height is constrained

    // Font sizes
    let label_size = (screen_height / 56) as f32; // Adjust based on screen size
    let tick_length = margin / 12;

    // Histogram area dimensions in effective coordinates
    let hist_screen_width = effective_width - margin * 2;
    let hist_screen_height = effective_height - margin * 2;

    // Oversampled histogram dimensions (only horizontal oversampling)
    let hist_oversampled_width = hist_screen_width * oversample;

    let mut channel_data = vec![vec![0u32; hist_oversampled_width]; channels];

    let half_width = raw_width / 2;
    let half_height = raw_height / 2;

    // Pre-calculate log scaling for horizontal axis
    let max_brightness_log = ((65536 - black_level as usize) as f64).log2();

    for h in 0..half_height {
        for w in 0..half_width {
            let sx = w * 2;
            let sy = h * 2;

            let tl = raw_data[sy * raw_width + sx];
            let tr = raw_data[sy * raw_width + sx + 1];
            let bl = raw_data[(sy + 1) * raw_width + sx];
            let br = raw_data[(sy + 1) * raw_width + sx + 1];

            // Demosaic based on bayer pattern
            let bayer_pixels = [tl, tr, bl, br];

            for i in 0..4 {
                let pixel_adj = bayer_pixels[i].saturating_sub(black_level) + 1;
                let bin = ((pixel_adj as f64).log2() * (hist_oversampled_width - 1) as f64
                    / max_brightness_log) as usize;

                match bayer_pattern[i] {
                    0 => channel_data[0][bin] += 2,
                    1 => channel_data[1][bin] += 1,
                    2 => channel_data[2][bin] += 2,
                    _ => {}
                }
            }
        }
    }

    // A/D values are integers, but the x-axis is log2(brightness), so adjacent integer codes map to bins that are far apart at the bright end and tightly packed at the dark end. Each occupied bin therefore represents a whole span of "missing" bins between it and the next reachable integer code; we divide its count by that span to get a density (counts-per-bin) so the curve isn't a comb of spikes. Previously this span was measured per frame as the distance to the nearest other occupied bin - but that distance jumps as scene noise changes WHICH sparse bins are hit, making bar heights flicker frame to frame. The span between adjacent integer codes is actually a fixed function of the bin position (the local stretch of the log mapping), so we compute it analytically here: scene-independent, identical every frame, no flicker.
    let mut bin_span = vec![1.0f64; hist_oversampled_width];
    let bins_per_log = (hist_oversampled_width - 1) as f64 / max_brightness_log;
    for w in 0..hist_oversampled_width {
        // Recover the brightness value at this bin (inverse of the log2 mapping used when binning), then measure how many bins one integer step (v -> v+1) covers locally. That width is the number of bins a single integer code "owns", i.e. the density divisor. Floored at 1.0 because at the dark end multiple integer codes share one bin (span < 1) and we must never divide the count by less than one bin's worth.
        let v = 2f64.powf(w as f64 / bins_per_log);
        let span = bins_per_log * ((v + 1.0) / v).log2();
        // .max(1.0): a sub-1 span at the dark end would inflate counts; clamp so the divisor is at least one bin. This is a value floor on a density divisor (not memory safety), justified because span < 1 means several codes fall in one bin and the natural divisor is then 1 bin, not a fraction.
        bin_span[w] = span.max(1.0);
    }

    // Use 3 channels for oversampled histogram (only the graph area)
    let mut histogram_oversampled = vec![0u8; hist_oversampled_width * hist_screen_height * 3];

    // Calculate max possible count for vertical scaling
    let max_possible_count = half_width * half_height * 2;
    let max_count_log = ((max_possible_count + 2) as f64).log2();

    let mut min_vals = vec![hist_oversampled_width; channels];
    let mut max_vals = vec![0; channels];
    let mut rem_vals = vec![0f64; channels];
    let mut height_vals = vec![hist_screen_height; channels];

    // Process each channel separately to maintain proper height value isolation
    for channel in 0..channels {
        for w in 0..hist_oversampled_width {
            if channel_data[channel][w] > 0 {
                if w < min_vals[channel] {
                    min_vals[channel] = w;
                }
                if w > max_vals[channel] {
                    max_vals[channel] = w;
                }
                let count_log =
                    (channel_data[channel][w] as f64 / bin_span[w] + 2.)
                        .log2();
                let height_normalized = count_log / max_count_log;
                rem_vals[channel] = height_normalized * hist_screen_height as f64;
                height_vals[channel] = hist_screen_height - rem_vals[channel] as usize;
            }
            for h in height_vals[channel]..hist_screen_height {
                let shade = 256 - (h - height_vals[channel]) * 256 / hist_screen_height;
                histogram_oversampled[(h * hist_oversampled_width + w) * 3 + channel] =
                    (shade * shade * 192 / 65536) as u8;
            }
            if height_vals[channel] > 0 {
                histogram_oversampled
                    [((height_vals[channel] - 1) * hist_oversampled_width + w) * 3 + channel] =
                    (rem_vals[channel].fract().sqrt() * 192.) as u8;
            }
        }
    }

    // Fill edges with gray in the oversampled histogram
    for h in 0..hist_screen_height {
        for channel in 0..channels {
            // Fill left edge
            for w in 0..min_vals[channel] {
                histogram_oversampled[(h * hist_oversampled_width + w) * 3 + channel] = fill;
            }
            // Fill right edge
            for w in (max_vals[channel] + 1)..hist_oversampled_width {
                histogram_oversampled[(h * hist_oversampled_width + w) * 3 + channel] = fill;
            }
        }
    }

    // Clear the pixels buffer (RGB format)
    pixels.fill(0);

    // Downsample the histogram area from oversampled to final resolution
    // Process all channels in unified loops
    for channel in 0..channels {
        for x in 0..hist_screen_width {
            for y in (0..hist_screen_height).rev() {
                let mut sum = 0u32;

                // Average all oversampled pixels for this output pixel
                for ox in 0..oversample {
                    let src_x = x * oversample + ox;
                    let src_idx = (y * hist_oversampled_width + src_x) * 3 + channel;
                    let val = histogram_oversampled[src_idx] as u32;
                    sum += val * val;
                }

                // Break if we hit zero
                if sum == 0 {
                    break;
                }

                // Place in final image with margins and apply rotation
                let effective_x = x + margin;
                let effective_y = y + margin;

                // Transform coordinates based on rotation
                let (dst_x, dst_y) = match rotation {
                    90 => (effective_y, effective_width - 1 - effective_x),
                    180 => (
                        effective_width - 1 - effective_x,
                        effective_height - 1 - effective_y,
                    ),
                    270 => (effective_height - 1 - effective_y, effective_x),
                    _ => (effective_x, effective_y), // 0 degrees
                };

                // Index into actual screen buffer
                let dst_idx = (dst_y * screen_width + dst_x) * 3 + channel;

                pixels[dst_idx] = isqrt16((sum / oversample as u32) as u16);
            }
        }
    }

    // Add gradient fade at margins for each channel independently
    for effective_y in margin..effective_height - margin {
        for channel in 0..channels {
            // Left margin gradient
            if min_vals[channel] > 0 {
                for effective_x in 0..margin {
                    // Transform coordinates based on rotation
                    let (dst_x, dst_y) = match rotation {
                        90 => (effective_y, effective_width - 1 - effective_x),
                        180 => (
                            effective_width - 1 - effective_x,
                            effective_height - 1 - effective_y,
                        ),
                        270 => (effective_height - 1 - effective_y, effective_x),
                        _ => (effective_x, effective_y), // 0 degrees
                    };
                    let dst_idx = (dst_y * screen_width + dst_x) * 3 + channel;
                    pixels[dst_idx] = (effective_x * fill as usize / margin) as u8;
                }
            }

            // Right margin - highlight if clipped, gradient if not
            if max_vals[channel] >= hist_oversampled_width - 1 {
                // Solid highlight warning
                for x in 0..margin {
                    let effective_x = effective_width - margin + x;
                    // Transform coordinates based on rotation
                    let (dst_x, dst_y) = match rotation {
                        90 => (effective_y, effective_width - 1 - effective_x),
                        180 => (
                            effective_width - 1 - effective_x,
                            effective_height - 1 - effective_y,
                        ),
                        270 => (effective_height - 1 - effective_y, effective_x),
                        _ => (effective_x, effective_y), // 0 degrees
                    };
                    let dst_idx = (dst_y * screen_width + dst_x) * 3 + channel;
                    pixels[dst_idx] = highlight;
                }
            } else if max_vals[channel] < hist_oversampled_width - 1 {
                // Gradient fade
                for x in 0..margin {
                    let effective_x = effective_width - margin + x;
                    // Transform coordinates based on rotation
                    let (dst_x, dst_y) = match rotation {
                        90 => (effective_y, effective_width - 1 - effective_x),
                        180 => (
                            effective_width - 1 - effective_x,
                            effective_height - 1 - effective_y,
                        ),
                        270 => (effective_height - 1 - effective_y, effective_x),
                        _ => (effective_x, effective_y), // 0 degrees
                    };
                    let dst_idx = (dst_y * screen_width + dst_x) * 3 + channel;
                    pixels[dst_idx] = ((margin - x - 1) * fill as usize / margin) as u8;
                }
            }
        }
    }

    // Border and axis line settings
    let border_value = if controls_visible { 64 } else { 191 };
    let axis_weight = 32u8;
    let tick_value = 191u8; // Same as border for consistency

    // Draw border lines (4 lines around the histogram)
    // Top border
    for effective_x in (margin - 1)..=(margin + hist_screen_width) {
        let effective_y = margin - 1;
        let (dst_x, dst_y) = match rotation {
            90 => (effective_y, effective_width - 1 - effective_x),
            180 => (
                effective_width - 1 - effective_x,
                effective_height - 1 - effective_y,
            ),
            270 => (effective_height - 1 - effective_y, effective_x),
            _ => (effective_x, effective_y), // 0 degrees
        };
        let idx = (dst_y * screen_width + dst_x) * 3;
        pixels[idx] = border_value;
        pixels[idx + 1] = border_value;
        pixels[idx + 2] = border_value;
    }

    // Bottom border
    for effective_x in (margin - 1)..=(margin + hist_screen_width) {
        let effective_y = margin + hist_screen_height;
        let (dst_x, dst_y) = match rotation {
            90 => (effective_y, effective_width - 1 - effective_x),
            180 => (
                effective_width - 1 - effective_x,
                effective_height - 1 - effective_y,
            ),
            270 => (effective_height - 1 - effective_y, effective_x),
            _ => (effective_x, effective_y), // 0 degrees
        };
        let idx = (dst_y * screen_width + dst_x) * 3;
        pixels[idx] = border_value;
        pixels[idx + 1] = border_value;
        pixels[idx + 2] = border_value;
    }

    // Left border
    for effective_y in (margin - 1)..=(margin + hist_screen_height) {
        let effective_x = margin - 1;
        let (dst_x, dst_y) = match rotation {
            90 => (effective_y, effective_width - 1 - effective_x),
            180 => (
                effective_width - 1 - effective_x,
                effective_height - 1 - effective_y,
            ),
            270 => (effective_height - 1 - effective_y, effective_x),
            _ => (effective_x, effective_y), // 0 degrees
        };
        let idx = (dst_y * screen_width + dst_x) * 3;
        pixels[idx] = border_value;
        pixels[idx + 1] = border_value;
        pixels[idx + 2] = border_value;
    }

    // Right border
    for effective_y in (margin - 1)..=(margin + hist_screen_height) {
        let effective_x = margin + hist_screen_width;
        let (dst_x, dst_y) = match rotation {
            90 => (effective_y, effective_width - 1 - effective_x),
            180 => (
                effective_width - 1 - effective_x,
                effective_height - 1 - effective_y,
            ),
            270 => (effective_height - 1 - effective_y, effective_x),
            _ => (effective_x, effective_y), // 0 degrees
        };
        let idx = (dst_y * screen_width + dst_x) * 3;
        pixels[idx] = border_value;
        pixels[idx + 1] = border_value;
        pixels[idx + 2] = border_value;
    }

    // Draw axis lines and add tick marks with labels

    // Vertical axis lines (brightness markers) with tick marks
    // Place lines at powers of 2 in brightness space
    let max_brightness = 65536 - black_level as usize;
    let max_power = (max_brightness as f64).log2().floor() as usize;

    for power in 0..=max_power {
        let brightness = 1 << power; // 2^power

        // Skip every other x-axis label when width is constrained
        if skip_x_labels && power % 2 == 1 {
            continue;
        }

        // Convert brightness to histogram bin position
        let x_pos =
            (brightness as f64).log2() * (hist_screen_width - 1) as f64 / max_brightness_log;
        let effective_x = margin + x_pos as usize;

        // Draw vertical line
        for effective_y in margin..(margin + hist_screen_height) {
            let (dst_x, dst_y) = match rotation {
                90 => (effective_y, effective_width - 1 - effective_x),
                180 => (
                    effective_width - 1 - effective_x,
                    effective_height - 1 - effective_y,
                ),
                270 => (effective_height - 1 - effective_y, effective_x),
                _ => (effective_x, effective_y), // 0 degrees
            };
            let idx = (dst_y * screen_width + dst_x) * 3;
            pixels[idx] += axis_weight;
            pixels[idx + 1] += axis_weight;
            pixels[idx + 2] += axis_weight;
        }

        // Draw tick mark and label based on controls visibility
        if !controls_visible {
            // Draw tick mark at bottom
            for tick_offset in 1..=tick_length {
                let effective_y = margin + hist_screen_height + tick_offset;
                let (dst_x, dst_y) = match rotation {
                    90 => (effective_y, effective_width - 1 - effective_x),
                    180 => (
                        effective_width - 1 - effective_x,
                        effective_height - 1 - effective_y,
                    ),
                    270 => (effective_height - 1 - effective_y, effective_x),
                    _ => (effective_x, effective_y), // 0 degrees
                };
                if dst_y < screen_height && dst_x < screen_width {
                    let idx = (dst_y * screen_width + dst_x) * 3;
                    pixels[idx] = tick_value;
                    pixels[idx + 1] = tick_value;
                    pixels[idx + 2] = tick_value;
                }
            }

            // Add label below tick
            let label = format!("{}", power);
            let label_y =
                margin + hist_screen_height + tick_length * 2 + (label_size * 0.5) as usize;
            let (dst_x, dst_y) = match rotation {
                90 => (label_y, effective_width - 1 - effective_x),
                180 => (
                    effective_width - 1 - effective_x,
                    effective_height - 1 - label_y,
                ),
                270 => (effective_height - 1 - label_y, effective_x),
                _ => (effective_x, label_y), // 0 degrees
            };
            text_renderer.draw_text_center(
                pixels,
                screen_width as u32,
                screen_height as u32,
                &label,
                dst_x as f32,
                dst_y as f32,
                label_size,
                300,
                192,
                192,
                192,
                rotation as u16,
            );
        }
    }

    // Horizontal axis lines (vertical position markers) with tick marks
    let v_stops = max_count_log as usize + 1;
    for stop in 1..=v_stops {
        // Skip every other y-axis label when height is constrained
        if skip_y_labels && stop % 2 == 0 {
            continue;
        }

        let effective_y = if controls_visible {
            // When controls are visible, invert the Y positions
            margin + (stop * hist_screen_height / v_stops) - 1
        } else {
            margin + hist_screen_height - (stop * hist_screen_height / v_stops)
        };

        // Draw horizontal line
        for effective_x in margin..(margin + hist_screen_width) {
            let (dst_x, dst_y) = match rotation {
                90 => (effective_y, effective_width - 1 - effective_x),
                180 => (
                    effective_width - 1 - effective_x,
                    effective_height - 1 - effective_y,
                ),
                270 => (effective_height - 1 - effective_y, effective_x),
                _ => (effective_x, effective_y), // 0 degrees
            };
            let idx = (dst_y * screen_width + dst_x) * 3;
            pixels[idx] += axis_weight;
            pixels[idx + 1] += axis_weight;
            pixels[idx + 2] += axis_weight;
        }

        if !controls_visible {
            // Draw tick mark at left
            for tick_offset in 1..tick_length {
                let effective_x = margin - tick_offset - 1;
                let (dst_x, dst_y) = match rotation {
                    90 => (effective_y, effective_width - 1 - effective_x),
                    180 => (
                        effective_width - 1 - effective_x,
                        effective_height - 1 - effective_y,
                    ),
                    270 => (effective_height - 1 - effective_y, effective_x),
                    _ => (effective_x, effective_y), // 0 degrees
                };
                if dst_x < screen_width && dst_y < screen_height {
                    let idx = (dst_y * screen_width + dst_x) * 3;
                    pixels[idx] = tick_value;
                    pixels[idx + 1] = tick_value;
                    pixels[idx + 2] = tick_value;
                }
            }
            // Add label to the left of tick
            let label = format!("{}", stop - 1);
            let label_x = margin - tick_length * 2 - (label_size * 0.5) as usize;
            let (dst_x, dst_y) = match rotation {
                90 => (effective_y, effective_width - 1 - label_x),
                180 => (
                    effective_width - 1 - label_x,
                    effective_height - 1 - effective_y,
                ),
                270 => (effective_height - 1 - effective_y, label_x),
                _ => (label_x, effective_y), // 0 degrees
            };
            text_renderer.draw_text_right(
                pixels,
                screen_width as u32,
                screen_height as u32,
                &label,
                dst_x as f32,
                dst_y as f32,
                label_size,
                300,
                192,
                192,
                192,
                rotation as u16,
            );
        }
    }
}

fn isqrt16(n: u16) -> u8 {
    if n < 2 {
        return n as u8;
    }

    let mut x = 0;
    let mut bit = 1 << (7 - (n.leading_zeros() >> 1));

    while bit != 0 {
        let guess = x | bit;
        if guess * guess <= n {
            x = guess;
        }
        bit >>= 1;
    }

    x as u8
}
