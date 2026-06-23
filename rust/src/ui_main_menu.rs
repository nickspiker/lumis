use crate::ui::text::TextRenderer;
use crate::CameraInfo;
use ndk::native_window::NativeWindow;
use ndk_sys::{ANativeWindow_Buffer, ANativeWindow_lock, ANativeWindow_unlockAndPost};
#[derive(Clone, Debug)]
pub enum MenuAction {
    StartCamera(usize),
    Exit,
}

#[derive(Clone)]
enum Button {
    Empty,
    Camera { index: usize, info: CameraInfo },
    Back,
    Exit,
}

#[derive(Clone, PartialEq)]
enum Screen {
    Main,            // one row per lens (group heads)
    Modes(i32),      // the capture modes of one lens (group_id)
}
pub struct MainMenu {
    width: u32,
    height: u32,
    x_margin: f32,
    y_margin: f32,
    button_height: f32,
    text_renderer: TextRenderer,
    buttons: Vec<Button>,
    pressed_button_index: Option<usize>,
    normal_buffer: Vec<u8>,
    pressed_buffer: Vec<u8>,
    density_dpi: f32,
    magic_counter: [u8; 3],
    last_pressed_button_index: Option<usize>,
    all_cameras: Vec<CameraInfo>, // full list incl. every mode
    screen: Screen,
    background: Vec<u8>, // pristine splash, used to clear buffers before re-rendering buttons
}
impl MainMenu {
    pub fn new(width: u32, height: u32, density_dpi: f32) -> Self {
        const SPLASH_PNG: &[u8] = include_bytes!("../assets/splash.png");
        let decoder = png::Decoder::new(SPLASH_PNG);
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut buf).unwrap();
        let info = reader.info();
        let img_width = info.width;
        let img_height = info.height;
        let scale = width as f32 / img_width as f32;
        let buffer_size = (width * height * 3) as usize;
        let mut normal_buffer = vec![0u8; buffer_size];
        let mut pressed_buffer = vec![0u8; buffer_size];
        for y in 0..height {
            for x in 0..width {
                let src_x = (x as f32 / scale) as u32;
                let src_y = (y as f32 / scale) as u32;
                if src_y < img_height {
                    let src_idx = ((src_y * img_width + src_x.min(img_width - 1)) * 3) as usize;
                    let dst_idx = (y * width + x) as usize * 3;
                    let pixels = &buf;
                    normal_buffer[dst_idx] = pixels[src_idx];
                    normal_buffer[dst_idx + 1] = pixels[src_idx + 1];
                    normal_buffer[dst_idx + 2] = pixels[src_idx + 2];
                    pressed_buffer[dst_idx] = pixels[src_idx];
                    pressed_buffer[dst_idx + 1] = pixels[src_idx + 1];
                    pressed_buffer[dst_idx + 2] = pixels[src_idx + 2];
                }
            }
        }
        // Keep a pristine copy of the splash background to clear button buffers before re-rendering on screen changes (otherwise old rows bleed through).
        let normal_buffer_bg = normal_buffer.clone();
        let mut menu = MainMenu {
            width,
            height,
            x_margin: width as f32 / 8.,
            y_margin: height as f32 / 12.,
            button_height: 0.,
            text_renderer: TextRenderer::new(),
            buttons: Vec::new(),
            pressed_button_index: None,
            normal_buffer,
            pressed_buffer,
            density_dpi,
            magic_counter: [0x06, 0x03, 0x16],
            last_pressed_button_index: None,
            all_cameras: Vec::new(),
            screen: Screen::Main,
            background: normal_buffer_bg,
        };
        menu.setup_buttons();
        menu
    }
    pub fn set_camera_list(&mut self, cameras: Vec<CameraInfo>) {
        self.all_cameras = cameras;
        self.screen = Screen::Main;
        self.rebuild_buttons();
    }

    // Rebuild the on-screen buttons for the current screen. Exit is always pinned to the last (bottom) slot; cameras fill upward from just above it, with empty padding on top so the list sits at the bottom.
    fn rebuild_buttons(&mut self) {
        self.buttons.clear();
        const NUM_SLOTS: usize = 10;
        self.button_height = (self.height as f32 - self.y_margin * 2.) / NUM_SLOTS as f32;

        // Pick the rows to show for this screen, each as (camera index, info).
        let rows: Vec<(usize, CameraInfo)> = match self.screen {
            Screen::Main => self
                .all_cameras
                .iter()
                // One row per lens: the group head (lowest-index member). Ungrouped cameras (group_id < 0) are their own head.
                .filter(|c| self.is_group_head(c))
                .map(|c| (c.index, c.clone()))
                .collect(),
            Screen::Modes(g) => self
                .all_cameras
                .iter()
                .filter(|c| c.group_id == g)
                .map(|c| (c.index, c.clone()))
                .collect(),
        };

        // A "Back" row occupies a slot on the sub-screen.
        let has_back = matches!(self.screen, Screen::Modes(_));
        let reserved = if has_back { 2 } else { 1 }; // Back (top of list) + Exit
        let max_rows = NUM_SLOTS - reserved;
        let rows_to_show = rows.len().min(max_rows);
        let empty_slots = NUM_SLOTS - reserved - rows_to_show;

        for _ in 0..empty_slots {
            self.buttons.push(Button::Empty);
        }
        if has_back {
            self.buttons.push(Button::Back);
        }
        for (index, info) in rows.into_iter().take(rows_to_show) {
            self.buttons.push(Button::Camera { index, info });
        }
        self.buttons.push(Button::Exit);
        self.render_buttons_to_buffers();
    }

    // First (highest-res) mode of a group is its head. We mark the head with mode_count = group size and the rest with 1, so head == "mode_count >= 1 and it's the first of its group in all_cameras". Since Kotlin emits the head first, the head is simply the earliest-index member of the group.
    fn is_group_head(&self, cam: &CameraInfo) -> bool {
        if cam.group_id < 0 {
            return true;
        }
        // Head = the lowest index among members sharing this group_id.
        let min_index = self
            .all_cameras
            .iter()
            .filter(|c| c.group_id == cam.group_id)
            .map(|c| c.index)
            .min()
            .unwrap_or(cam.index);
        cam.index == min_index
    }
    fn setup_buttons(&mut self) {
        const NUM_SLOTS: usize = 10;
        self.button_height = (self.height as f32 - self.y_margin * 2.) / NUM_SLOTS as f32;
        for _ in 0..NUM_SLOTS - 1 {
            self.buttons.push(Button::Empty);
        }
        self.buttons.push(Button::Exit);
        self.render_buttons_to_buffers();
    }
    fn render_buttons_to_buffers(&mut self) {
        // Clear to the pristine splash first so rows removed on a screen change (e.g. when fewer buttons are shown) don't leave stale pixels behind.
        self.normal_buffer.copy_from_slice(&self.background);
        self.pressed_buffer.copy_from_slice(&self.background);
        for i in 0..self.buttons.len() {
            let y = self.y_margin + i as f32 * self.button_height;
            self.draw_button(i, y, false);
            self.draw_button(i, y, true);
        }
    }

    fn draw_button(&mut self, button_index: usize, y: f32, pressed: bool) {
        match &self.buttons[button_index] {
            Button::Empty => {}
            Button::Camera { .. } => {
                self.draw_camera_button(button_index, y, pressed);
            }
            Button::Back => {
                self.draw_back_button(y, pressed);
            }
            Button::Exit => {
                self.draw_exit_button(y, pressed);
            }
        }
    }
    fn format_exposure_time(nanos: i64) -> String {
        let seconds = nanos as f32 / 1000000000.;
        if seconds >= 1. {
            Self::format_sig_figs(seconds, 3) + "s"
        } else {
            let denominator = (1. / seconds).round() as i32;
            format!("1/{}", denominator)
        }
    }
    fn format_sig_figs(value: f32, sig_figs: usize) -> String {
        if value == 0. {
            return "0".to_string();
        }
        let magnitude = value.abs().log10();
        let digits = sig_figs as i32 - magnitude.floor() as i32 - 1;
        let scaled = (value * 10f32.powi(digits)).round() / 10f32.powi(digits);
        if magnitude >= -3. && magnitude < 6. {
            let decimal_places = (sig_figs as i32 - magnitude.floor() as i32 - 1).max(0) as usize;
            format!("{:.*}", decimal_places, scaled)
        } else {
            // Proper scientific notation: a×10ⁿ
            let exponent = scaled.abs().log10().floor() as i32;
            let mantissa = scaled / 10f32.powi(exponent);
            if exponent >= 0 {
                format!("{:.2}×10^{}", mantissa, exponent)
            } else {
                format!("{:.2}×10^{{{}}}", mantissa, exponent) // Negative exponents in braces
            }
        }
    }
    fn draw_bayer_square(
        pixels: &mut [u8],
        pattern: i32,
        x: f32,
        y: f32,
        size: f32,
        width: u32,
        height: u32,
    ) {
        let half_size = size / 2.;
        let quarter_size = size / 4.;
        let colours = match pattern {
            0 => [(255, 0, 0), (0, 255, 0), (0, 255, 0), (0, 0, 255)],
            1 => [(0, 255, 0), (255, 0, 0), (0, 0, 255), (0, 255, 0)],
            2 => [(0, 255, 0), (0, 0, 255), (255, 0, 0), (0, 255, 0)],
            3 => [(0, 0, 255), (0, 255, 0), (0, 255, 0), (255, 0, 0)],
            _ => [
                (128, 128, 128),
                (128, 128, 128),
                (128, 128, 128),
                (128, 128, 128),
            ],
        };
        let positions = [
            (x - quarter_size, y - quarter_size),
            (x + quarter_size, y - quarter_size),
            (x - quarter_size, y + quarter_size),
            (x + quarter_size, y + quarter_size),
        ];
        for (i, &(cx, cy)) in positions.iter().enumerate() {
            let colour = colours[i];
            let x_start = (cx - quarter_size + 1.) as i32;
            let x_end = (cx + quarter_size - 1.) as i32;
            let y_start = (cy - quarter_size + 1.) as i32;
            let y_end = (cy + quarter_size - 1.) as i32;
            for py in y_start.max(0)..y_end.min(height as i32) {
                for px in x_start.max(0)..x_end.min(width as i32) {
                    let offset = (py as usize * width as usize + px as usize) * 3;
                    pixels[offset] = colour.0;
                    pixels[offset + 1] = colour.1;
                    pixels[offset + 2] = colour.2;
                }
            }
        }
        let grid_colour = (64, 64, 64);
        let vx = x as i32;
        for py in ((y - half_size) as i32)..((y + half_size) as i32) {
            if py >= 0 && py < height as i32 && vx >= 0 && vx < width as i32 {
                let offset = (py as usize * width as usize + vx as usize) * 3;
                pixels[offset] = grid_colour.0;
                pixels[offset + 1] = grid_colour.1;
                pixels[offset + 2] = grid_colour.2;
            }
        }
        let hy = y as i32;
        for px in ((x - half_size) as i32)..((x + half_size) as i32) {
            if hy >= 0 && hy < height as i32 && px >= 0 && px < width as i32 {
                let offset = (hy as usize * width as usize + px as usize) * 3;
                pixels[offset] = grid_colour.0;
                pixels[offset + 1] = grid_colour.1;
                pixels[offset + 2] = grid_colour.2;
            }
        }
    }
    fn draw_camera_button(&mut self, button_index: usize, y: f32, pressed: bool) {
        // "multiple" is only meaningful on the main screen; inside a lens's mode list each row shows its own resolution.
        let on_main = self.screen == Screen::Main;
        let pixels = if pressed {
            &mut self.pressed_buffer
        } else {
            &mut self.normal_buffer
        };
        let button_margin = 3. / self.button_height;
        let x_start;
        let x_end;
        let y_start;
        let y_end;
        if pressed {
            x_start = (self.x_margin * 0.995) as i32;
            x_end = (self.width as f32 - self.x_margin * 0.995) as i32;
            y_start = (y - self.button_height * 0.005) as i32;
            y_end = (y + self.button_height * 1.005) as i32;
        } else {
            x_start = self.x_margin as i32;
            x_end = (self.width as f32 - self.x_margin) as i32;
            y_start = y as i32;
            y_end = (y + self.button_height) as i32;
        }
        let x_center = (x_start + x_end) / 2;
        let y_center = (y_start + y_end) / 2;
        let rgb = match &self.buttons[button_index] {
            Button::Camera { info, .. } => {
                if info.supports_raw {
                    // The button's identity colour comes from its hardware level (unchanged from before). On top of that we MODULATE red/green to signal the capture mode, leaving blue (and the overall identity) intact:
                    //   RED   boosted when this is a cropped sub-FOV readout.
                    //   GREEN boosted when this is the full-res (max-res, non-binned) readout.
                    // A plain binned full-FOV entry keeps its original colour exactly.
                    let base_colour = match info.hardware_level {
                        0 => [0xFF, 0xA0, 0x60],
                        1 => [0x60, 0xFF, 0x60],
                        2 => [0xFF, 0x60, 0x60],
                        3 => [0x60, 0x60, 0xFF],
                        4 => [0x60, 0xFF, 0xFF],
                        _ => [0xFF, 0xFF, 0x60],
                    };
                    let base_colour = [
                        if info.is_cropped { 0xFF } else { base_colour[0] },
                        if info.max_res { 0xFF } else { base_colour[1] },
                        base_colour[2],
                    ];
                    if !pressed {
                        [
                            ((base_colour[0] as u16 * 192) >> 8) as u8,
                            ((base_colour[1] as u16 * 192) >> 8) as u8,
                            ((base_colour[2] as u16 * 192) >> 8) as u8,
                        ]
                    } else {
                        base_colour
                    }
                } else {
                    if pressed {
                        [0x80, 0x80, 0x80]
                    } else {
                        [0xC0, 0xC0, 0xC0]
                    }
                }
            }
            _ => [0, 0, 0],
        };
        let f_rgb = [
            2. - rgb[0] as f32 / 512.,
            2. - rgb[1] as f32 / 512.,
            2. - rgb[2] as f32 / 512.,
        ];
        for py in y_start..y_center {
            for px in x_start..x_center {
                let x_weight = (1. - (px - x_start) as f32 * button_margin).max(0.);
                let y_weight = (1. - (py - y_start) as f32 * button_margin).max(0.);
                let weight = x_weight * x_weight * x_weight * x_weight * x_weight
                    + y_weight * y_weight * y_weight * y_weight * y_weight;
                let mut weight_r = weight * weight * weight * 10.;
                weight_r = weight_r * weight_r * weight_r;
                weight_r = 1. - (weight_r - 0.5).abs() * (f_rgb[0]);
                let mut weight_g = weight * weight * weight * 8.;
                weight_g = weight_g * weight_g * weight_g;
                weight_g = 1. - (weight_g - 0.5).abs() * (f_rgb[1]);
                let mut weight_b = weight * weight * weight * 5.;
                weight_b = weight_b * weight_b * weight_b;
                weight_b = 1. - (weight_b - 0.5).abs() * (f_rgb[2]);
                let mut weight_a = weight * weight * weight * 5.;
                weight_a = weight_a * weight_a * weight_a;
                weight_a = (weight_a - 0.04).abs() * (1.75);
                let offset = (py as usize * self.width as usize + px as usize) * 3;
                pixels[offset] = (weight_r.max(0.) * rgb[0] as f32
                    + pixels[offset] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 1] = (weight_g.max(0.) * rgb[1] as f32
                    + pixels[offset + 1] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 2] = (weight_b.max(0.) * rgb[2] as f32
                    + pixels[offset + 2] as f32 * weight_a.min(1.))
                    as u8;
            }
        }
        for py in y_start..y_center {
            for px in x_center..x_end {
                let x_weight = (1. - (x_end - px - 1) as f32 * button_margin).max(0.);
                let y_weight = (1. - (py - y_start) as f32 * button_margin).max(0.);
                let weight = x_weight * x_weight * x_weight * x_weight * x_weight
                    + y_weight * y_weight * y_weight * y_weight * y_weight;
                let mut weight_r = weight * weight * weight * 10.;
                weight_r = weight_r * weight_r * weight_r;
                weight_r = 1. - (weight_r - 0.5).abs() * (f_rgb[0]);
                let mut weight_g = weight * weight * weight * 8.;
                weight_g = weight_g * weight_g * weight_g;
                weight_g = 1. - (weight_g - 0.5).abs() * (f_rgb[1]);
                let mut weight_b = weight * weight * weight * 5.;
                weight_b = weight_b * weight_b * weight_b;
                weight_b = 1. - (weight_b - 0.5).abs() * (f_rgb[2]);
                let mut weight_a = weight * weight * weight * 5.;
                weight_a = weight_a * weight_a * weight_a;
                weight_a = (weight_a - 0.04).abs() * (1.75);
                let offset = (py as usize * self.width as usize + px as usize) * 3;
                pixels[offset] = (weight_r.max(0.) * rgb[0] as f32
                    + pixels[offset] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 1] = (weight_g.max(0.) * rgb[1] as f32
                    + pixels[offset + 1] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 2] = (weight_b.max(0.) * rgb[2] as f32
                    + pixels[offset + 2] as f32 * weight_a.min(1.))
                    as u8;
            }
        }
        for py in y_center..y_end {
            for px in x_start..x_center {
                let x_weight = (1. - (px - x_start) as f32 * button_margin).max(0.);
                let y_weight = (1. - (y_end - py - 1) as f32 * button_margin).max(0.);
                let weight = x_weight * x_weight * x_weight * x_weight * x_weight
                    + y_weight * y_weight * y_weight * y_weight * y_weight;
                let mut weight_r = weight * weight * weight * 10.;
                weight_r = weight_r * weight_r * weight_r;
                weight_r = 1. - (weight_r - 0.5).abs() * (f_rgb[0]);
                let mut weight_g = weight * weight * weight * 8.;
                weight_g = weight_g * weight_g * weight_g;
                weight_g = 1. - (weight_g - 0.5).abs() * (f_rgb[1]);
                let mut weight_b = weight * weight * weight * 5.;
                weight_b = weight_b * weight_b * weight_b;
                weight_b = 1. - (weight_b - 0.5).abs() * (f_rgb[2]);
                let mut weight_a = weight * weight * weight * 5.;
                weight_a = weight_a * weight_a * weight_a;
                weight_a = (weight_a - 0.04).abs() * (1.75);
                let offset = (py as usize * self.width as usize + px as usize) * 3;
                pixels[offset] = (weight_r.max(0.) * rgb[0] as f32
                    + pixels[offset] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 1] = (weight_g.max(0.) * rgb[1] as f32
                    + pixels[offset + 1] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 2] = (weight_b.max(0.) * rgb[2] as f32
                    + pixels[offset + 2] as f32 * weight_a.min(1.))
                    as u8;
            }
        }
        for py in y_center..y_end {
            for px in x_center..x_end {
                let x_weight = (1. - (x_end - px - 1) as f32 * button_margin).max(0.);
                let y_weight = (1. - (y_end - py - 1) as f32 * button_margin).max(0.);
                let weight = x_weight * x_weight * x_weight * x_weight * x_weight
                    + y_weight * y_weight * y_weight * y_weight * y_weight;
                let mut weight_r = weight * weight * weight * 10.;
                weight_r = weight_r * weight_r * weight_r;
                weight_r = 1. - (weight_r - 0.5).abs() * (f_rgb[0]);
                let mut weight_g = weight * weight * weight * 8.;
                weight_g = weight_g * weight_g * weight_g;
                weight_g = 1. - (weight_g - 0.5).abs() * (f_rgb[1]);
                let mut weight_b = weight * weight * weight * 5.;
                weight_b = weight_b * weight_b * weight_b;
                weight_b = 1. - (weight_b - 0.5).abs() * (f_rgb[2]);
                let mut weight_a = weight * weight * weight * 5.;
                weight_a = weight_a * weight_a * weight_a;
                weight_a = (weight_a - 0.04).abs() * (1.75);
                let offset = (py as usize * self.width as usize + px as usize) * 3;
                pixels[offset] = (weight_r.max(0.) * rgb[0] as f32
                    + pixels[offset] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 1] = (weight_g.max(0.) * rgb[1] as f32
                    + pixels[offset + 1] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 2] = (weight_b.max(0.) * rgb[2] as f32
                    + pixels[offset + 2] as f32 * weight_a.min(1.))
                    as u8;
            }
        }
        if let Button::Camera { info, .. } = &self.buttons[button_index] {
            let button_width = self.width as f32 - 2. * self.x_margin;
            let sensor_diagonal_mm = (info.sensor_width * info.sensor_width
                + info.sensor_height * info.sensor_height)
                .sqrt();
            let fov_diagonal = if !info.focal_lengths.is_empty()
                && info.sensor_width > 0.
                && info.sensor_height > 0.
            {
                let focal_length = info.focal_lengths[0];
                2. * (sensor_diagonal_mm / (2. * focal_length))
                    .atan()
                    .to_degrees()
            } else {
                0.
            };
            let pixel_size_um = if info.sensor_width > 0. && info.pixel_array_width > 0 {
                (info.sensor_width * 1000.) / info.pixel_array_width as f32
            } else {
                0.
            };
            let line_spacing = self.button_height * 0.025;
            let top_margin = self.button_height * 0.12;
            let mut colour_themes = vec![
                ((0xFF, 0x90, 0xB0), 400),
                ((0x90, 0xB0, 0xFF), 400),
                ((0xFF, 0xC0, 0x90), 400),
                ((0x90, 0xFF, 0x90), 500),
                ((0xFF, 0x90, 0xC0), 500),
                ((0x90, 0xFF, 0xC0), 500),
                ((0xFF, 0xE0, 0x90), 500),
                ((0xB0, 0x90, 0xFF), 600),
                ((0x90, 0xFF, 0xB0), 500),
                ((0xFF, 0xB0, 0xB0), 600),
                ((0x90, 0xA0, 0xFF), 600),
                ((0xFF, 0xFF, 0x70), 600),
                ((0xB0, 0xFF, 0x90), 600),
            ];
            if !pressed {
                for theme in &mut colour_themes {
                    theme.0 = (
                        ((theme.0 .0 as u16 * 205) >> 8) as u8,
                        ((theme.0 .1 as u16 * 205) >> 8) as u8,
                        ((theme.0 .2 as u16 * 205) >> 8) as u8,
                    );
                    theme.1 -= 100;
                }
            }
            let line1_size = self.button_height * 0.25;
            let line1_y = y + top_margin + line1_size * 0.5;
            let left_gap = button_width * 0.03;
            let right_gap = button_width * 0.03;
            let megapixels = (info.width * info.height) as f32 / 1_000_000.;
            // A lens with several capture modes shows "multiple" instead of a single resolution; tapping it (future) opens a per-mode picker.
            let mp_text = if on_main && info.mode_count > 1 {
                "multiple ".to_string()
            } else {
                format!("{}mp ", Self::format_sig_figs(megapixels, 3))
            };
            let mp_length = self.text_renderer.draw_text_left(
                pixels,
                self.width,
                self.height,
                &mp_text,
                self.x_margin + left_gap,
                line1_y,
                line1_size,
                colour_themes[1].1,
                colour_themes[1].0 .0,
                colour_themes[1].0 .1,
                colour_themes[1].0 .2,
                0,
            );
            let facing_str = match info.facing {
                0 => "Front",
                1 => "Back",
                2 => "External",
                _ => "Unknown",
            };
            self.text_renderer.draw_text_left(
                pixels,
                self.width,
                self.height,
                facing_str,
                self.x_margin + left_gap + mp_length,
                line1_y,
                line1_size,
                colour_themes[0].1,
                colour_themes[0].0 .0,
                colour_themes[0].0 .1,
                colour_themes[0].0 .2,
                0,
            );
            if fov_diagonal > 0. {
                let fov_text = format!("{}°", fov_diagonal as i32);
                let fov_size = self.text_renderer.draw_text_right(
                    pixels,
                    self.width,
                    self.height,
                    &fov_text,
                    self.width as f32 - self.x_margin - right_gap,
                    line1_y,
                    line1_size,
                    colour_themes[2].1,
                    colour_themes[2].0 .0,
                    colour_themes[2].0 .1,
                    colour_themes[2].0 .2,
                    0,
                );
                Self::draw_fov_wedge(
                    pixels,
                    self.width,
                    self.width as f32 - self.x_margin - right_gap - fov_size - line1_size,
                    line1_y,
                    line1_size,
                    fov_diagonal,
                    (255, 255, 255),
                )
            }
            let line2_size = self.button_height * 0.17;
            let line2_y = line1_y + line1_size / 2. + line_spacing + line2_size / 2.;
            let min_shutter = Self::format_exposure_time(info.min_exposure);
            let max_shutter = Self::format_exposure_time(info.max_exposure);
            let shutter_text = format!("{}-{}s", min_shutter, max_shutter);
            self.text_renderer.draw_text_left(
                pixels,
                self.width,
                self.height,
                &shutter_text,
                self.x_margin + left_gap,
                line2_y,
                line2_size,
                colour_themes[3].1,
                colour_themes[3].0 .0,
                colour_themes[3].0 .1,
                colour_themes[3].0 .2,
                0,
            );
            let aperture_text = if info.apertures.is_empty() {
                "f/?".to_string()
            } else if info.apertures.len() == 1 {
                format!("f/{}", Self::format_sig_figs(info.apertures[0], 2))
            } else {
                let min_f = info
                    .apertures
                    .iter()
                    .min_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();
                let max_f = info
                    .apertures
                    .iter()
                    .max_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();
                format!(
                    "f/{}-f/{}",
                    Self::format_sig_figs(*min_f, 2),
                    Self::format_sig_figs(*max_f, 2)
                )
            };
            let aperature_width = self.text_renderer.draw_text_right(
                pixels,
                self.width,
                self.height,
                &aperture_text,
                self.width as f32 - self.x_margin - right_gap,
                line2_y,
                line2_size,
                colour_themes[5].1,
                colour_themes[5].0 .0,
                colour_themes[5].0 .1,
                colour_themes[5].0 .2,
                0,
            );
            let focal_text = if info.focal_lengths.is_empty() {
                "?mm ".to_string()
            } else if info.focal_lengths.len() == 1 {
                format!("{}mm ", Self::format_sig_figs(info.focal_lengths[0], 3))
            } else {
                let min_f = info
                    .focal_lengths
                    .iter()
                    .min_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();
                let max_f = info
                    .focal_lengths
                    .iter()
                    .max_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();
                format!(
                    "{}-{}mm ",
                    Self::format_sig_figs(*min_f, 3),
                    Self::format_sig_figs(*max_f, 3)
                )
            };
            self.text_renderer.draw_text_right(
                pixels,
                self.width,
                self.height,
                &focal_text,
                self.width as f32 - self.x_margin - right_gap - aperature_width,
                line2_y,
                line2_size,
                colour_themes[4].1,
                colour_themes[4].0 .0,
                colour_themes[4].0 .1,
                colour_themes[4].0 .2,
                0,
            );
            let line3_size = self.button_height * 0.16;
            let line3_y = line2_y + line2_size / 2. + line_spacing + line3_size / 2.;
            let iso_text = format!("ISO {}-{}", info.min_iso, info.max_iso);
            self.text_renderer.draw_text_left(
                pixels,
                self.width,
                self.height,
                &iso_text,
                self.x_margin + left_gap,
                line3_y,
                line3_size,
                colour_themes[6].1,
                colour_themes[6].0 .0,
                colour_themes[6].0 .1,
                colour_themes[6].0 .2,
                0,
            );
            Self::draw_sensor_rectangle(
                pixels,
                self.width,
                self.height,
                self.density_dpi,
                (x_center as f32) + button_width / 16.,
                y_center as f32,
                info.sensor_width,
                info.sensor_height,
                colour_themes[7].0 .0,
                colour_themes[7].0 .1,
                colour_themes[7].0 .2,
                pressed,
            );
            let diag_text = format!("{}mm", Self::format_sig_figs(sensor_diagonal_mm, 2));
            let mm_to_pixels = self.density_dpi / 25.4;
            let sensor_height_pixels = info.sensor_height * mm_to_pixels;
            let text_size = sensor_height_pixels / 3.;
            self.text_renderer.draw_text_center(
                pixels,
                self.width,
                self.height,
                &diag_text,
                (x_center as f32) + button_width / 16.,
                y_center as f32,
                text_size,
                colour_themes[7].1,
                colour_themes[7].0 .0,
                colour_themes[7].0 .1,
                colour_themes[7].0 .2,
                0,
            );
            let focus_text = if info.min_focus_distance == 0. {
                "∞".to_string()
            } else {
                let distance_cm = 100. / info.min_focus_distance;
                format!("∞-{}cm", Self::format_sig_figs(distance_cm, 3))
            };
            self.text_renderer.draw_text_right(
                pixels,
                self.width,
                self.height,
                &focus_text,
                self.width as f32 - self.x_margin - right_gap,
                line3_y,
                line3_size,
                colour_themes[8].1,
                colour_themes[8].0 .0,
                colour_themes[8].0 .1,
                colour_themes[8].0 .2,
                0,
            );
            let line4_size = self.button_height * 0.14;
            let line4_y = line3_y + line3_size / 2. + line_spacing + line4_size / 2.;
            let bayer_x = self.x_margin + left_gap + line4_size * 0.6;
            Self::draw_bayer_square(
                pixels,
                info.bayer_pattern,
                bayer_x,
                line4_y - line4_size * 0.05,
                line4_size * 0.75,
                self.width,
                self.height,
            );
            let bw_text = format!("b{} w{}", info.black_level, info.white_level);
            let bw_length = self.text_renderer.draw_text_left(
                pixels,
                self.width,
                self.height,
                &bw_text,
                bayer_x + line4_size * 0.7,
                line4_y,
                line4_size,
                colour_themes[9].1,
                colour_themes[9].0 .0,
                colour_themes[9].0 .1,
                colour_themes[9].0 .2,
                0,
            );
            let pixel_text = format!(" {}μm", Self::format_sig_figs(pixel_size_um, 3));
            self.text_renderer.draw_text_left(
                pixels,
                self.width,
                self.height,
                &pixel_text,
                bayer_x + line4_size * 0.7 + bw_length,
                line4_y,
                line4_size,
                colour_themes[10].1,
                colour_themes[10].0 .0,
                colour_themes[10].0 .1,
                colour_themes[10].0 .2,
                0,
            );
            let api_text = match info.hardware_level {
                0 => "LIMITED",
                1 => "RAW",
                2 => "LEGACY",
                3 => "RAW+",
                _ => "EXTERNAL",
            };
            let api_length = self.text_renderer.draw_text_right(
                pixels,
                self.width,
                self.height,
                api_text,
                self.width as f32 - self.x_margin - right_gap,
                line4_y,
                line4_size,
                colour_themes[12].1,
                colour_themes[12].0 .0,
                colour_themes[12].0 .1,
                colour_themes[12].0 .2,
                0,
            );
            let dimensions_text = format!("{}×{} ", info.width, info.height);
            self.text_renderer.draw_text_right(
                pixels,
                self.width,
                self.height,
                &dimensions_text,
                self.width as f32 - self.x_margin - right_gap - api_length,
                line4_y,
                line4_size,
                colour_themes[11].1,
                colour_themes[11].0 .0,
                colour_themes[11].0 .1,
                colour_themes[11].0 .2,
                0,
            );
        }
    }
    fn draw_sensor_rectangle(
        pixels: &mut Vec<u8>,
        screen_width: u32,
        screen_height: u32,
        density_dpi: f32,
        center_x: f32,
        center_y: f32,
        sensor_width_mm: f32,
        sensor_height_mm: f32,
        r: u8,
        g: u8,
        b: u8,
        pressed: bool,
    ) {
        let mm_to_pixels = density_dpi / 25.4;
        let rect_width = sensor_width_mm * mm_to_pixels;
        let rect_height = sensor_height_mm * mm_to_pixels;
        let x1_f = center_x - rect_width / 2.;
        let x2_f = center_x + rect_width / 2.;
        let y1_f = center_y - rect_height / 2.;
        let y2_f = center_y + rect_height / 2.;
        let x1 = x1_f as i32;
        let x2 = x2_f as i32;
        let y1 = y1_f as i32;
        let y2 = y2_f as i32;
        let (rb, gb, bb) = if pressed {
            (0x50, 0x40, 0x50)
        } else {
            (0x40, 0x30, 0x40)
        };
        for y in y1..y2 {
            for x in x1..x2 {
                if x >= 0 && x < screen_width as i32 && y >= 0 && y < screen_height as i32 {
                    let offset = (y as usize * screen_width as usize + x as usize) * 3;
                    let bayer_x = (x - x1) % 4;
                    let bayer_y = (y - y1) % 4;
                    let (r, g, b) = match (bayer_y / 2, bayer_x / 2) {
                        (0, 0) => (rb, 0, 0),
                        (0, 1) => (0, gb, 0),
                        (1, 0) => (0, gb, 0),
                        (1, 1) => (0, 0, bb),
                        _ => (0, 0, 0),
                    };
                    pixels[offset] = r;
                    pixels[offset + 1] = g;
                    pixels[offset + 2] = b;
                }
            }
        }
        let gamma = 2.2;
        let colour = vec![r as f32 / 256., g as f32 / 256., b as f32 / 256., 1.];
        let gap = 0.5;
        let xl = x1_f * gap + center_x * (1. - gap);
        let xr = x2_f * gap + center_x * (1. - gap);
        let yl = y2_f * gap + center_y * (1. - gap);
        let yr = y1_f * gap + center_y * (1. - gap);
        Self::draw_line_u8(
            pixels,
            screen_width as usize,
            3,
            x1_f,
            y2_f,
            xl,
            yl,
            colour.clone(),
            colour.clone(),
            gamma,
        );
        Self::draw_line_u8(
            pixels,
            screen_width as usize,
            3,
            xr,
            yr,
            x2_f,
            y1_f,
            colour.clone(),
            colour.clone(),
            gamma,
        );
        let diag_length = ((x2_f - x1_f) * (x2_f - x1_f) + (y1_f - y2_f) * (y1_f - y2_f)).sqrt();
        let arrowhead_length = diag_length / 12.;
        let diag_angle = (rect_height / rect_width).atan();
        let angle1 = diag_angle + 22.5_f32.to_radians();
        let angle2 = diag_angle - 22.5_f32.to_radians();
        Self::draw_line_u8(
            pixels,
            screen_width as usize,
            3,
            x2_f,
            y1_f,
            x2_f - arrowhead_length * angle1.cos(),
            y1_f + arrowhead_length * angle1.sin(),
            colour.clone(),
            colour.clone(),
            gamma,
        );
        Self::draw_line_u8(
            pixels,
            screen_width as usize,
            3,
            x2_f,
            y1_f,
            x2_f - arrowhead_length * angle2.cos(),
            y1_f + arrowhead_length * angle2.sin(),
            colour.clone(),
            colour.clone(),
            gamma,
        );
        Self::draw_line_u8(
            pixels,
            screen_width as usize,
            3,
            x1_f,
            y2_f,
            x1_f + arrowhead_length * angle1.cos(),
            y2_f - arrowhead_length * angle1.sin(),
            colour.clone(),
            colour.clone(),
            gamma,
        );
        Self::draw_line_u8(
            pixels,
            screen_width as usize,
            3,
            x1_f,
            y2_f,
            x1_f + arrowhead_length * angle2.cos(),
            y2_f - arrowhead_length * angle2.sin(),
            colour.clone(),
            colour.clone(),
            gamma,
        );
        let edge = if pressed { 192 } else { 128 };
        for x in (x1 - 1)..=(x2 + 1) {
            if x >= 0 && x < screen_width as i32 && (y1 - 1) >= 0 && (y1 - 1) < screen_height as i32
            {
                let offset = ((y1 - 1) as usize * screen_width as usize + x as usize) * 3;
                pixels[offset] = edge;
                pixels[offset + 1] = edge;
                pixels[offset + 2] = edge;
            }
        }
        for x in (x1 - 1)..=(x2 + 1) {
            if x >= 0 && x < screen_width as i32 && (y2 + 1) >= 0 && (y2 + 1) < screen_height as i32
            {
                let offset = ((y2 + 1) as usize * screen_width as usize + x as usize) * 3;
                pixels[offset] = edge;
                pixels[offset + 1] = edge;
                pixels[offset + 2] = edge;
            }
        }
        for y in (y1 - 1)..=(y2 + 1) {
            if (x1 - 1) >= 0 && (x1 - 1) < screen_width as i32 && y >= 0 && y < screen_height as i32
            {
                let offset = (y as usize * screen_width as usize + (x1 - 1) as usize) * 3;
                pixels[offset] = edge;
                pixels[offset + 1] = edge;
                pixels[offset + 2] = edge;
            }
        }
        for y in (y1 - 1)..=(y2 + 1) {
            if (x2 + 1) >= 0 && (x2 + 1) < screen_width as i32 && y >= 0 && y < screen_height as i32
            {
                let offset = (y as usize * screen_width as usize + (x2 + 1) as usize) * 3;
                pixels[offset] = edge;
                pixels[offset + 1] = edge;
                pixels[offset + 2] = edge;
            }
        }
    }
    fn draw_fov_wedge(
        pixels: &mut Vec<u8>,
        width: u32,
        x: f32,
        y: f32,
        size: f32,
        fov_degrees: f32,
        colour: (u8, u8, u8),
    ) {
        let radius = size * 0.5;
        let fov_radians = fov_degrees.to_radians();
        let half_fov = fov_radians / 2.;

        // Define the bounding box for our circle
        let x_start = (x - radius).max(0.) as i32;
        let x_end = (x + radius).min(width as f32 - 1.) as i32;
        let y_start = (y - radius).max(0.) as i32;
        let y_end = (y + radius).min((pixels.len() / (width as usize * 3)) as f32 - 1.) as i32;

        // Draw the wedge gradient
        for py in y_start..=y_end {
            for px in x_start..=x_end {
                // Calculate distance from center
                let dx = px as f32 - x;
                let dy = py as f32 - y;
                let distance_squared = dx * dx + dy * dy;

                // Skip if outside the circle
                if distance_squared > radius * radius {
                    continue;
                }

                // Calculate angle from center (0° is pointing up, negative Y direction)
                // In screen coordinates, negative Y is up, so we use -dy
                let angle = dx.atan2(-dy);

                // Check if angle is within the FOV wedge
                // Normalize angle to [-π, π] range
                let normalized_angle = if angle.abs() <= half_fov {
                    angle
                } else if angle > std::f32::consts::PI - half_fov {
                    angle - 2. * std::f32::consts::PI
                } else if angle < -std::f32::consts::PI + half_fov {
                    angle + 2. * std::f32::consts::PI
                } else {
                    angle
                };

                // Check if within FOV wedge
                if normalized_angle.abs() > half_fov {
                    continue;
                }

                // Calculate gradient intensity (255 at center, 0 at edge)
                let intensity = ((1. - distance_squared / (radius * radius)).powi(2) * 256.) as u8;

                // Apply the colour with intensity
                let offset = (py as usize * width as usize + px as usize) * 3;
                if offset + 2 < pixels.len() {
                    // Blend with existing pixel based on intensity
                    let blend_factor = intensity as f32 / 256.;
                    pixels[offset] = (colour.0 as f32 * blend_factor
                        + pixels[offset] as f32 * (1. - blend_factor))
                        as u8;
                    pixels[offset + 1] = (colour.1 as f32 * blend_factor
                        + pixels[offset + 1] as f32 * (1. - blend_factor))
                        as u8;
                    pixels[offset + 2] = (colour.2 as f32 * blend_factor
                        + pixels[offset + 2] as f32 * (1. - blend_factor))
                        as u8;
                }
            }
        }

        // Draw the edge lines using the existing draw_line_u8 function
        let white = vec![1., 1., 1., 1.];
        let transparent = vec![1., 1., 1., 0.];

        // Calculate edge line positions
        // Left edge (negative angle)
        let left_angle = -half_fov;
        let left_sin = left_angle.sin();
        let left_cos = left_angle.cos();

        // Points along the left edge
        let left_at_radius_x = x + radius * left_sin;
        let left_at_radius_y = y - radius * left_cos;
        let left_extended_x = x + radius * 1.5 * left_sin;
        let left_extended_y = y - radius * 1.5 * left_cos;

        // Right edge (positive angle)
        let right_angle = half_fov;
        let right_sin = right_angle.sin();
        let right_cos = right_angle.cos();

        // Points along the right edge
        let right_at_radius_x = x + radius * right_sin;
        let right_at_radius_y = y - radius * right_cos;
        let right_extended_x = x + radius * 1.5 * right_sin;
        let right_extended_y = y - radius * 1.5 * right_cos;

        // Draw left edge: center to radius (solid white)
        Self::draw_line_u8(
            pixels,
            width as usize,
            3,
            x,
            y,
            left_at_radius_x,
            left_at_radius_y,
            white.clone(),
            white.clone(),
            2.,
        );

        // Draw left edge: radius to extended (white to transparent)
        Self::draw_line_u8(
            pixels,
            width as usize,
            3,
            left_at_radius_x,
            left_at_radius_y,
            left_extended_x,
            left_extended_y,
            white.clone(),
            transparent.clone(),
            2.,
        );

        // Draw right edge: center to radius (solid white)
        Self::draw_line_u8(
            pixels,
            width as usize,
            3,
            x,
            y,
            right_at_radius_x,
            right_at_radius_y,
            white.clone(),
            white.clone(),
            2.,
        );

        // Draw right edge: radius to extended (white to transparent)
        Self::draw_line_u8(
            pixels,
            width as usize,
            3,
            right_at_radius_x,
            right_at_radius_y,
            right_extended_x,
            right_extended_y,
            white.clone(),
            transparent.clone(),
            2.,
        );
    }
    fn draw_line_u8(
        image: &mut Vec<u8>,
        image_width: usize,
        channels: usize,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        colour_start: Vec<f32>,
        colour_end: Vec<f32>,
        gamma: f32,
    ) {
        let total_distance = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
        let steep = (y1 - y0).abs() > (x1 - x0).abs();
        let (mut x0, mut y0, mut x1, mut y1) = (x0, y0, x1, y1);
        if steep {
            std::mem::swap(&mut x0, &mut y0);
            std::mem::swap(&mut x1, &mut y1);
        }
        let (colour_start, colour_end) = if x0 > x1 {
            std::mem::swap(&mut x0, &mut x1);
            std::mem::swap(&mut y0, &mut y1);
            // Also swap the colours to maintain gradient direction
            (colour_end, colour_start)
        } else {
            (colour_start, colour_end)
        };
        let dx = x1 - x0;
        let dy = y1 - y0;
        let gradient = if dx == 0. { 1. } else { dy / dx };
        let xend = (x0 + 0.5).floor();
        let yend = y0 + gradient * (xend - x0);
        let xgap = 1. - (x0 + 0.5).fract();
        let xpxl1 = xend;
        let ypxl1 = yend.floor();

        let mut plot = |x: isize, y: isize, c: f32, blend_factor: f32| {
            // Interpolate colour between start and end
            let colour = colour_end
                .iter()
                .zip(colour_start.iter())
                .map(|(end, start)| start * (1. - blend_factor) + end * blend_factor)
                .collect::<Vec<f32>>();

            if x >= 0
                && x < image_width as isize
                && y >= 0
                && y < (image.len() / (image_width * channels)) as isize
            {
                let idx = (y as usize) * image_width + (x as usize);
                let alias = c.powf(1f32 / gamma);

                // Check if we have an extra element for alpha
                let alpha = if colour.len() > channels {
                    colour[channels] * alias
                } else {
                    alias
                };

                // Simple alpha blending for all channels
                for channel in 0..channels {
                    if channel < colour.len() {
                        let src = colour[channel] * 255.0;
                        let dst = image[idx * channels + channel] as f32;
                        image[idx * channels + channel] =
                            (src * alpha + dst * (1.0 - alpha)).clamp(0.0, 255.0) as u8;
                    }
                }
            }
        };

        if steep {
            plot(
                ypxl1 as isize,
                xpxl1 as isize,
                (1. - yend.fract()) * xgap,
                0.,
            );
            plot(
                (ypxl1 + 1.) as isize,
                xpxl1 as isize,
                yend.fract() * xgap,
                0.,
            );
        } else {
            plot(
                xpxl1 as isize,
                ypxl1 as isize,
                (1. - yend.fract()) * xgap,
                0.,
            );
            plot(
                xpxl1 as isize,
                (ypxl1 + 1.) as isize,
                yend.fract() * xgap,
                0.,
            );
        }
        let mut intery = yend + gradient;
        let xend = (x1 + 0.5).floor();
        let yend = y1 + gradient * (xend - x1);
        let xgap = (x1 + 0.5).fract();
        let xpxl2 = xend;
        let ypxl2 = yend.floor();
        if steep {
            plot(
                ypxl2 as isize,
                xpxl2 as isize,
                (1. - yend.fract()) * xgap,
                1.,
            );
            plot(
                (ypxl2 + 1.) as isize,
                xpxl2 as isize,
                yend.fract() * xgap,
                1.,
            );
        } else {
            plot(
                xpxl2 as isize,
                ypxl2 as isize,
                (1. - yend.fract()) * xgap,
                1.,
            );
            plot(
                xpxl2 as isize,
                (ypxl2 + 1.) as isize,
                yend.fract() * xgap,
                1.,
            );
        }
        if steep {
            for x in (xpxl1 as isize + 1)..(xpxl2 as isize) {
                let current_distance = ((x as f32 - x0).powi(2) + (intery - y0).powi(2)).sqrt();
                let blend_factor = current_distance / total_distance;
                plot(
                    intery.floor() as isize,
                    x,
                    1. - intery.fract(),
                    blend_factor,
                );
                plot(
                    (intery.floor() + 1.) as isize,
                    x,
                    intery.fract(),
                    blend_factor,
                );
                intery += gradient;
            }
        } else {
            for x in (xpxl1 as isize + 1)..(xpxl2 as isize) {
                let current_distance = ((x as f32 - x0).powi(2) + (intery - y0).powi(2)).sqrt();
                let blend_factor = current_distance / total_distance;
                plot(
                    x,
                    intery.floor() as isize,
                    1. - intery.fract(),
                    blend_factor,
                );
                plot(
                    x,
                    (intery.floor() + 1.) as isize,
                    intery.fract(),
                    blend_factor,
                );
                intery += gradient;
            }
        }
    }
    fn draw_exit_button(&mut self, y: f32, pressed: bool) {
        let pixels = if pressed {
            &mut self.pressed_buffer
        } else {
            &mut self.normal_buffer
        };
        let button_margin = 3. / self.button_height;
        let highlight = if pressed { 0.1 } else { 0. };
        let x_start;
        let x_end;
        let y_start;
        let y_end;
        if pressed {
            x_start = (self.x_margin * 0.995) as i32;
            x_end = (self.width as f32 - self.x_margin * 0.995) as i32;
            y_start = (y - self.button_height * 0.005) as i32;
            y_end = (y + self.button_height * 1.005) as i32;
        } else {
            x_start = self.x_margin as i32;
            x_end = (self.width as f32 - self.x_margin) as i32;
            y_start = y as i32;
            y_end = (y + self.button_height) as i32;
        }
        let x_center = (x_start + x_end) / 2;
        let y_center = (y_start + y_end) / 2;
        for py in y_start..y_center {
            for px in x_start..x_center {
                let x_weight = (1. - (px - x_start) as f32 * button_margin).max(0.);
                let y_weight = (1. - (py - y_start) as f32 * button_margin).max(0.);
                let weight = x_weight * x_weight * x_weight * x_weight * x_weight
                    + y_weight * y_weight * y_weight * y_weight * y_weight;
                let mut weight_r = weight * weight * weight * 8.;
                weight_r = weight_r * weight_r * weight_r;
                weight_r = 1. - (weight_r - 0.5).abs() * (1.65 - highlight);
                let mut weight_g = weight * weight * weight * 5.;
                weight_g = weight_g * weight_g * weight_g;
                weight_g = 1. - (weight_g - 0.5).abs() * (1.8 - highlight);
                let mut weight_a = weight * weight * weight * 5.;
                weight_a = weight_a * weight_a * weight_a;
                weight_a = (weight_a - 0.04).abs() * (1.75 - highlight);
                let offset = (py as usize * self.width as usize + px as usize) * 3;
                pixels[offset] = (weight_r.max(0.) * 0x100 as f32
                    + pixels[offset] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 1] = (weight_g.max(0.) * 0x40 as f32
                    + pixels[offset + 1] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 2] = (pixels[offset + 2] as f32 * weight_a.min(1.)) as u8;
            }
        }
        for py in y_start..y_center {
            for px in x_center..x_end {
                let x_weight = (1. - (x_end - px - 1) as f32 * button_margin).max(0.);
                let y_weight = (1. - (py - y_start) as f32 * button_margin).max(0.);
                let weight = x_weight * x_weight * x_weight * x_weight * x_weight
                    + y_weight * y_weight * y_weight * y_weight * y_weight;
                let mut weight_r = weight * weight * weight * 8.;
                weight_r = weight_r * weight_r * weight_r;
                weight_r = 1. - (weight_r - 0.5).abs() * (1.65 - highlight);
                let mut weight_g = weight * weight * weight * 5.;
                weight_g = weight_g * weight_g * weight_g;
                weight_g = 1. - (weight_g - 0.5).abs() * (1.8 - highlight);
                let mut weight_a = weight * weight * weight * 5.;
                weight_a = weight_a * weight_a * weight_a;
                weight_a = (weight_a - 0.04).abs() * (1.75 - highlight);
                let offset = (py as usize * self.width as usize + px as usize) * 3;
                pixels[offset] = (weight_r.max(0.) * 0x100 as f32
                    + pixels[offset] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 1] = (weight_g.max(0.) * 0x40 as f32
                    + pixels[offset + 1] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 2] = (pixels[offset + 2] as f32 * weight_a.min(1.)) as u8;
            }
        }
        for py in y_center..y_end {
            for px in x_start..x_center {
                let x_weight = (1. - (px - x_start) as f32 * button_margin).max(0.);
                let y_weight = (1. - (y_end - py - 1) as f32 * button_margin).max(0.);
                let weight = x_weight * x_weight * x_weight * x_weight * x_weight
                    + y_weight * y_weight * y_weight * y_weight * y_weight;
                let mut weight_r = weight * weight * weight * 8.;
                weight_r = weight_r * weight_r * weight_r;
                weight_r = 1. - (weight_r - 0.5).abs() * (1.65 - highlight);
                let mut weight_g = weight * weight * weight * 5.;
                weight_g = weight_g * weight_g * weight_g;
                weight_g = 1. - (weight_g - 0.5).abs() * (1.8 - highlight);
                let mut weight_a = weight * weight * weight * 5.;
                weight_a = weight_a * weight_a * weight_a;
                weight_a = (weight_a - 0.04).abs() * (1.75 - highlight);
                let offset = (py as usize * self.width as usize + px as usize) * 3;
                pixels[offset] = (weight_r.max(0.) * 0x100 as f32
                    + pixels[offset] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 1] = (weight_g.max(0.) * 0x40 as f32
                    + pixels[offset + 1] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 2] = (pixels[offset + 2] as f32 * weight_a.min(1.)) as u8;
            }
        }
        for py in y_center..y_end {
            for px in x_center..x_end {
                let x_weight = (1. - (x_end - px - 1) as f32 * button_margin).max(0.);
                let y_weight = (1. - (y_end - py - 1) as f32 * button_margin).max(0.);
                let weight = x_weight * x_weight * x_weight * x_weight * x_weight
                    + y_weight * y_weight * y_weight * y_weight * y_weight;
                let mut weight_r = weight * weight * weight * 8.;
                weight_r = weight_r * weight_r * weight_r;
                weight_r = 1. - (weight_r - 0.5).abs() * (1.65 - highlight);
                let mut weight_g = weight * weight * weight * 5.;
                weight_g = weight_g * weight_g * weight_g;
                weight_g = 1. - (weight_g - 0.5).abs() * (1.8 - highlight);
                let mut weight_a = weight * weight * weight * 5.;
                weight_a = weight_a * weight_a * weight_a;
                weight_a = (weight_a - 0.04).abs() * (1.75 - highlight);
                let offset = (py as usize * self.width as usize + px as usize) * 3;
                pixels[offset] = (weight_r.max(0.) * 0x100 as f32
                    + pixels[offset] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 1] = (weight_g.max(0.) * 0x40 as f32
                    + pixels[offset + 1] as f32 * weight_a.min(1.))
                    as u8;
                pixels[offset + 2] = (pixels[offset + 2] as f32 * weight_a.min(1.)) as u8;
            }
        }
        if pressed {
            self.text_renderer.draw_text_center(
                pixels,
                self.width,
                self.height,
                "EXIT",
                self.width as f32 / 2.,
                y + self.button_height / 2.,
                self.button_height * 0.5,
                300,
                255,
                128,
                64,
                0,
            );
        } else {
            self.text_renderer.draw_text_center(
                pixels,
                self.width,
                self.height,
                "EXIT",
                self.width as f32 / 2.,
                y + self.button_height / 2.,
                self.button_height * 0.5,
                200,
                255,
                64,
                0,
                0,
            );
        }
    }

    // "Back" button for the mode sub-screen. Same rounded shape as Exit but tinted teal/blue (channels swapped) with a "< BACK" label.
    fn draw_back_button(&mut self, y: f32, pressed: bool) {
        let pixels = if pressed {
            &mut self.pressed_buffer
        } else {
            &mut self.normal_buffer
        };
        let button_margin = 3. / self.button_height;
        let highlight = if pressed { 0.1 } else { 0. };
        let (x_start, x_end, y_start, y_end) = if pressed {
            (
                (self.x_margin * 0.995) as i32,
                (self.width as f32 - self.x_margin * 0.995) as i32,
                (y - self.button_height * 0.005) as i32,
                (y + self.button_height * 1.005) as i32,
            )
        } else {
            (
                self.x_margin as i32,
                (self.width as f32 - self.x_margin) as i32,
                y as i32,
                (y + self.button_height) as i32,
            )
        };
        let x_center = (x_start + x_end) / 2;
        let y_center = (y_start + y_end) / 2;
        let row_stride = self.width as usize;
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
            let offset = (py as usize * row_stride + px as usize) * 3;
            // Teal: weak red, strong green+blue (vs Exit's strong red).
            pixels[offset] = (pixels[offset] as f32 * wa.min(1.)) as u8;
            pixels[offset + 1] =
                (wg.max(0.) * 0xE0 as f32 + pixels[offset + 1] as f32 * wa.min(1.)) as u8;
            pixels[offset + 2] =
                (wc.max(0.) * 0x100 as f32 + pixels[offset + 2] as f32 * wa.min(1.)) as u8;
        };
        // Top-left quadrant
        for py in y_start..y_center {
            for px in x_start..x_center {
                let xw = (1. - (px - x_start) as f32 * button_margin).max(0.);
                let yw = (1. - (py - y_start) as f32 * button_margin).max(0.);
                shade(px, py, xw, yw);
            }
        }
        // Top-right quadrant
        for py in y_start..y_center {
            for px in x_center..x_end {
                let xw = (1. - (x_end - px - 1) as f32 * button_margin).max(0.);
                let yw = (1. - (py - y_start) as f32 * button_margin).max(0.);
                shade(px, py, xw, yw);
            }
        }
        // Bottom-left quadrant
        for py in y_center..y_end {
            for px in x_start..x_center {
                let xw = (1. - (px - x_start) as f32 * button_margin).max(0.);
                let yw = (1. - (y_end - py - 1) as f32 * button_margin).max(0.);
                shade(px, py, xw, yw);
            }
        }
        // Bottom-right quadrant
        for py in y_center..y_end {
            for px in x_center..x_end {
                let xw = (1. - (x_end - px - 1) as f32 * button_margin).max(0.);
                let yw = (1. - (y_end - py - 1) as f32 * button_margin).max(0.);
                shade(px, py, xw, yw);
            }
        }
        let (a, r, g, b) = if pressed { (300u16, 64u8, 255u8, 255u8) } else { (200u16, 0u8, 200u8, 255u8) };
        self.text_renderer.draw_text_center(
            pixels,
            self.width,
            self.height,
            "< BACK",
            self.width as f32 / 2.,
            y + self.button_height / 2.,
            self.button_height * 0.5,
            a,
            r,
            g,
            b,
            0,
        );
    }
    fn has_button_state_changed(&self) -> bool {
        self.pressed_button_index != self.last_pressed_button_index
    }

    pub fn draw(&mut self, window: &NativeWindow, full_draw: bool) {
        // Update button state tracking
        self.last_pressed_button_index = self.pressed_button_index;

        unsafe {
            let mut buffer = std::mem::zeroed::<ANativeWindow_Buffer>();
            let lock_result = ANativeWindow_lock(
                window.ptr().as_ptr(),
                &mut buffer as *mut _,
                std::ptr::null_mut(),
            );
            if lock_result < 0 {
                // Window not ready, returning early
                return;
            }
            let stride = buffer.stride as usize;
            let width = buffer.width as u32;
            let height = buffer.height as u32;
            let pixels = std::slice::from_raw_parts_mut(
                buffer.bits as *mut u8,
                stride * height as usize * 3,
            );

            // Check magic pixel for buffer validation (top-right corner)
            let tr_idx = ((stride - 1) * 3) as usize;
            let tr_r = pixels[tr_idx];
            let tr_g = pixels[tr_idx + 1];
            let tr_b = pixels[tr_idx + 2];

            // Check blue first (most likely to change), then green, then red
            let needs_full_render = full_draw
                || tr_b != self.magic_counter[2]
                || tr_g != self.magic_counter[1]
                || tr_r != self.magic_counter[0];

            if needs_full_render {
                // Menu buffer validation failed, doing full render
                // Increment counter for new render using branchless approach
                let value = ((self.magic_counter[0] as u32) << 16)
                    | ((self.magic_counter[1] as u32) << 8)
                    | (self.magic_counter[2] as u32);
                let new_value = (value + 1) & 0xFFFFFF;
                self.magic_counter[0] = (new_value >> 16) as u8;
                self.magic_counter[1] = (new_value >> 8) as u8;
                self.magic_counter[2] = new_value as u8;

                // Do full menu render
                for y in 0..height {
                    let button_index = if y >= self.y_margin as u32
                        && y < (self.height as f32 - self.y_margin) as u32
                    {
                        Some(((y as f32 - self.y_margin) / self.button_height) as usize)
                    } else {
                        None
                    };
                    let source_buffer = if let Some(idx) = button_index {
                        if self.pressed_button_index == Some(idx) && idx < self.buttons.len() {
                            &self.pressed_buffer
                        } else {
                            &self.normal_buffer
                        }
                    } else {
                        &self.normal_buffer
                    };
                    let src_offset = y as usize * self.width as usize * 3;
                    let dst_offset = y as usize * stride * 3;
                    let copy_size = ((self.width * 3).min(width * 3)) as usize;
                    if src_offset + copy_size <= source_buffer.len() {
                        let src_slice = &source_buffer[src_offset..src_offset + copy_size];
                        let dst_slice = &mut pixels[dst_offset..dst_offset + copy_size];
                        dst_slice.copy_from_slice(src_slice);
                    }
                } // Close the for y in 0..height loop

                // Write magic pixel after full render
                pixels[tr_idx] = self.magic_counter[0]; // Red
                pixels[tr_idx + 1] = self.magic_counter[1]; // Green
                pixels[tr_idx + 2] = self.magic_counter[2]; // Blue
            } else {
                // Menu buffer is valid, do partial render (only if button state changed)
                if self.has_button_state_changed() {
                    // Only render the changed button areas, not the full menu
                    for y in 0..height {
                        let button_index = if y >= self.y_margin as u32
                            && y < (self.height as f32 - self.y_margin) as u32
                        {
                            Some(((y as f32 - self.y_margin) / self.button_height) as usize)
                        } else {
                            None
                        };
                        let source_buffer = if let Some(idx) = button_index {
                            if self.pressed_button_index == Some(idx) && idx < self.buttons.len() {
                                &self.pressed_buffer
                            } else {
                                &self.normal_buffer
                            }
                        } else {
                            &self.normal_buffer
                        };
                        let src_offset = y as usize * self.width as usize * 3;
                        let dst_offset = y as usize * stride * 3;
                        let copy_size = ((self.width * 3).min(width * 3)) as usize;
                        if src_offset + copy_size <= source_buffer.len() {
                            let src_slice = &source_buffer[src_offset..src_offset + copy_size];
                            let dst_slice = &mut pixels[dst_offset..dst_offset + copy_size];
                            dst_slice.copy_from_slice(src_slice);
                        }
                    }
                }
                // No magic pixel update needed for partial render
            }

            ANativeWindow_unlockAndPost(window.ptr().as_ptr());
        }
    }

    pub fn handle_touch(&mut self, action: i32, x: f32, y: f32) -> (Option<MenuAction>, bool) {
        let previous_pressed = self.pressed_button_index;
        let menu_action = match action {
            0 => {
                if x < self.x_margin || x > self.width as f32 - self.x_margin {
                    return (None, false);
                }
                if y < self.y_margin || y > self.height as f32 - self.y_margin {
                    return (None, false);
                }
                let button_index = ((y - self.y_margin) / self.button_height) as usize;
                if button_index < self.buttons.len() {
                    self.pressed_button_index = Some(button_index);
                }
                None
            }
            2 => {
                if self.pressed_button_index.is_some() {
                    let in_margins = x >= self.x_margin
                        && x <= self.width as f32 - self.x_margin
                        && y >= self.y_margin
                        && y <= self.height as f32 - self.y_margin;
                    if in_margins {
                        let current_button = ((y - self.y_margin) / self.button_height) as usize;
                        if self.pressed_button_index != Some(current_button) {
                            self.pressed_button_index = None;
                        }
                    } else {
                        self.pressed_button_index = None;
                    }
                }
                None
            }
            1 => {
                if let Some(idx) = self.pressed_button_index {
                    if x >= self.x_margin
                        && x <= self.width as f32 - self.x_margin
                        && y >= self.y_margin
                        && y <= self.height as f32 - self.y_margin
                    {
                        let current_button = ((y - self.y_margin) / self.button_height) as usize;
                        if current_button == idx && idx < self.buttons.len() {
                            // navigate flag: when set, we changed screen and must rebuild+redraw.
                            let mut navigate: Option<Screen> = None;
                            let act = match &self.buttons[idx] {
                                Button::Camera { index, info } => {
                                    if !info.supports_raw {
                                        None
                                    } else if self.screen == Screen::Main && info.mode_count > 1 {
                                        // Multi-mode lens on the main screen: open the mode sub-picker instead of starting the camera.
                                        navigate = Some(Screen::Modes(info.group_id));
                                        None
                                    } else {
                                        // Single mode, or already on the sub-screen: start it.
                                        Some(MenuAction::StartCamera(*index))
                                    }
                                }
                                Button::Back => {
                                    navigate = Some(Screen::Main);
                                    None
                                }
                                Button::Exit => Some(MenuAction::Exit),
                                Button::Empty => None,
                            };
                            self.pressed_button_index = None;
                            if let Some(target) = navigate {
                                self.screen = target;
                                self.rebuild_buttons();
                                // Invalidate the magic pixel so the next draw is a FULL redraw (Kotlin always calls draw with full_draw=false, and the whole button set just changed - a partial draw would leave the old screen's rows on screen).
                                self.magic_counter[0] = self.magic_counter[0].wrapping_add(1);
                                return (None, true);
                            }
                            act
                        } else {
                            self.pressed_button_index = None;
                            None
                        }
                    } else {
                        self.pressed_button_index = None;
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        };
        let state_changed = previous_pressed != self.pressed_button_index;
        (menu_action, state_changed)
    }
}
