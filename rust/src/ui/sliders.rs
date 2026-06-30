use crate::ui::ui::UserInterface;

pub fn get_slider_labels(
    ui: &UserInterface,
    slider_index: usize,
) -> (
    (String, [u8; 3]),
    (String, [u8; 3]),
    (String, [u8; 3]),
    (String, [u8; 3]),
) {
    match slider_index {
        4 => {
            // Use exposure time setting from UI
            let exposure_time = ui.format_time(ui.exposure_time_ms);
            // Top-left label: normally the slider's time-base span. In slitscan, show how long a FULL ring
            // takes to fill instead - the ring holds ring_rows/period time-columns (= width for standard
            // Bayer, width/2 for quad), and each column is one exposure (or one frame at 1/FPS when the
            // exposure time is 0/free-running). So you can read off how long the whole strip takes to scroll.
            let is_slitscan = crate::shared_memory::RawMode::from(
                ui.header[crate::shared_memory::CURRENT_MODE_IDX] as u8,
            ) == crate::shared_memory::RawMode::Slitscan;
            let top_left = if is_slitscan {
                let w = ui.sensor_x_size.max(1);
                let ring_rows = ui.slitscan_buffer.len() / w;
                let period = if ui.header[crate::shared_memory::QUAD_BAYER_IDX] != 0 { 4 } else { 2 };
                let columns = (ring_rows / period) as u64;
                let per_column_ms = if ui.exposure_time_ms > 0 {
                    ui.exposure_time_ms
                } else {
                    let fps = f64::from_bits(ui.header[crate::shared_memory::FPS_IDX]);
                    if fps > 0. { (1000.0 / fps) as u64 } else { 0 }
                };
                ui.format_time(columns.saturating_mul(per_column_ms))
            } else {
                ui.format_time(ui.time_base.duration_ms() as u64)
            };

            (
                (top_left, [253, 128, 255]),
                (exposure_time, [253, 128, 255]),
                ("".to_string(), [253, 128, 255]), // No elapsed/remaining for slider labels
                ("".to_string(), [253, 128, 255]), // They get calculated on the fly
            )
        }
        3 => {
            // ISO - log scale calculation inline
            let current_iso = ui.min_iso * (ui.max_iso / ui.min_iso).powf(ui.iso_slider);
            (
                ("ISO".to_string(), [255, 151, 128]),
                (format!("{:.0}", current_iso), [255, 151, 128]),
                (format!("{:.0}", ui.min_iso), [255, 151, 128]),
                (format!("{:.0}", ui.max_iso), [255, 151, 128]),
            )
        }
        2 => {
            // Shutter - log scale calculation inline
            let shutter_ns =
                ui.min_shutter_ns * (ui.max_shutter_ns / ui.min_shutter_ns).powf(ui.shutter_slider);

            let format_shutter = |ns: f64| {
                let ms = ns / 1_000_000.0;
                if ms < 1000.0 {
                    let shutter_fraction = (1000.0 / ms) as i32;
                    format!("1/{}s", shutter_fraction)
                } else {
                    format!("{}s", ms / 1000.0)
                }
            };

            (
                ("SHUTTER".to_string(), [206, 255, 128]),
                (format_shutter(shutter_ns), [206, 255, 128]),
                (format_shutter(ui.min_shutter_ns), [206, 255, 128]),
                (format_shutter(ui.max_shutter_ns), [206, 255, 128]),
            )
        }
        1 => {
            // Focus - linear scale calculation inline
            let focus = ui.focus_slider * ui.min_focus_distance;
            let format_focus = |f: f64| {
                if f == 0.0 {
                    "∞".to_string()
                } else {
                    let distance_m = 1.0 / f;
                    if distance_m < 1.0 {
                        format!("{:.0}cm", distance_m * 100.0)
                    } else {
                        format!("{:.1}m", distance_m)
                    }
                }
            };

            (
                ("FOCUS".to_string(), [128, 255, 202]),
                (format_focus(focus), [128, 255, 202]),
                (format_focus(0.0), [128, 255, 202]),
                (format_focus(ui.min_focus_distance), [128, 255, 202]),
            )
        }
        0 => {
            let gain_stops = ui.display_gain.log2();
            (
                ("GAIN".to_string(), [128, 155, 255]),
                (format!("{:.2}", gain_stops), [128, 155, 255]),
                ("0.00".to_string(), [128, 155, 255]),
                ("12.00".to_string(), [128, 155, 255]), // 2^12 = 12 stops
            )
        }
        _ => (
            ("".to_string(), [0xFF, 0xFF, 0xFF]),
            ("".to_string(), [0xFF, 0xFF, 0xFF]),
            ("".to_string(), [0xFF, 0xFF, 0xFF]),
            ("".to_string(), [0xFF, 0xFF, 0xFF]),
        ),
    }
}
