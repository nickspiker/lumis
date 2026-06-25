use crate::image::integrator::TimeBase;
use crate::shared_memory::*;
use crate::ui::arrows::ArrowBuffers;
use crate::ui::buttons;
use crate::ui::screen::draw_screen;
use crate::ui::text::TextRenderer;
use crate::ui::touch::*;
use crate::ui::ui::ui_constants::CALIBRATION_BUTTON_SIZE;
use arc_swap::ArcSwap;
use chameleon::{
    encode_settings, get_settings, scan_target, verichrome_dir, write_settings, ImageData, RawInfo,
};
use log::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use RawMode;

/// Camera/LMS working RGB -> linear Rec.2020 RGB. Decoded (BLAKE3-validated) from the
/// Chameleon "rec2020-1.mag" terminal profile. Row-major: output R,G,B each a dot product
/// with the input [r,g,b]. The display surface is tagged BT.2020 so Android performs the
/// final Rec.2020 -> panel-gamut conversion.
pub const REC2020_DISPLAY_MATRIX: [f32; 9] = [
    3.168_241,
    -2.156_882_8,
    0.096_456_88,
    -0.266_362_52,
    1.404_945_7,
    -0.175_554_8,
    0.003_891_53,
    -0.020_567_68,
    0.945_832_6,
];

pub mod ui_constants {
    pub const LONG_EDGE_MARGIN: f32 = 1. / 24.;
    pub const SHORT_EDGE_MARGIN: f32 = 1. / 12.;
    pub const LABEL_WIDTH: f32 = 1. / 5.;
    pub const LABEL_HEIGHT: f32 = 1. / 4.;
    pub const LABEL_OFFSET: f32 = 1. / 9.;
    pub const LABEL_VGAP: f32 = 0.2;
    pub const ARROW_SIZE: f32 = 0.5;
    pub const ARROW_OFFSET: f32 = 1. / 6.;
    pub const SKINNY_BUTTON_WIDTH: f32 = 1. / 3.;
    pub const FAT_BUTTON_WIDTH: f32 = 1. / 6.;
    pub const CALIBRATION_BUTTON_SIZE: f32 = 1. / 7.;
    pub const CONTROLS_HEIGHT_FAT: f32 = 0.9;
    pub const CONTROLS_HEIGHT_SKINNY: f32 = 0.5;
    pub const TOTAL_TRACKS: usize = 8;
    pub const SLIDER_CIRCLE_DIAGONAL: f32 = 1. / 216.;
    pub const DRAG_THRESHOLD: f32 = 1. / 38.;
    pub const CLIPPING_THRESHOLD: u16 = 0xF000;

    pub const BUTTON_COLOURS: [(f32, f32, f32); 4] = [
        (0.7, 0.5, 0.9), // Histogram
        (0.6, 0.9, 0.5), // Time
        (0.5, 0.8, 0.9), // Mode
        (0.9, 0.5, 0.4), // Exit
    ];
}

pub struct UserInterface {
    pub screen_run: usize,
    pub screen_rise: usize,
    pub screen_aspect: f32,
    pub sensor_x_size: usize,
    pub sensor_y_size: usize,
    pub x_margin: f32,
    pub y_margin: f32,
    pub sensor_orientation: i32,
    pub camera_facing: i32,
    pub device_rotation: u16,
    pub sensor_white_level: u16,
    pub sensor_black_level: u16,
    pub bayer_pattern: u32,
    // Shared memory slices - read-only references
    pub raw_average: &'static [u16],
    pub raw_difference: &'static [u16],
    pub raw_black_level: u16,
    pub display_gain: f64,
    pub min_iso: f64,
    pub max_iso: f64,
    pub min_shutter_ns: f64,
    pub max_shutter_ns: f64,
    pub min_focus_distance: f64,
    pub exposure_time_ms: u64,

    // Slider state tracking (all 0 to 1)
    pub exposure_time_slider: f64,   // 4: exposure time
    pub iso_slider: f64,             // 3: ISO
    pub shutter_slider: f64,         // 2: shutter speed
    pub focus_slider: f64,           // 1: focus distance
    pub gain_slider: f64,            // 0: display gain
    pub active_track: Option<usize>, // Which track is currently being used

    // UI visibility and view mode
    pub controls_visible: bool, // Show/hide controls and clipping overlay
    pub view_1to1: bool,        // 1:1 pixel view mode
    pub pan_offset_x: f32,      // Pan offset in 1:1 mode
    pub pan_offset_y: f32,      // Pan offset in 1:1 mode
    pub touch_start_x: f32,     // Touch start for drag detection
    pub touch_start_y: f32,     // Touch start for drag detection
    pub last_touch_x: f32,      // Last valid touch coordinates (for TouchAction::Up)
    pub last_touch_y: f32,      // Last valid touch coordinates (for TouchAction::Up)
    pub is_dragging: bool,      // Currently dragging
    pub touch_is_dead: bool,    // Touch started in margin (dead zone)
    pub last_touch_valid: bool, // Whether previous touch was valid (for state transitions)

    pub text_renderer: TextRenderer,

    // Track-based arrow press state
    pub pressed_arrow: Option<(usize, bool)>, // (track_index, is_increase)

    // Button states
    pub active_button: Option<usize>, // Which button (0-3) is currently pressed
    pub histogram_visible: bool,      // Whether histogram overlay is visible

    // Histogram calculation
    pub histogram_buffer: Arc<ArcSwap<Option<Vec<u8>>>>, // Atomic histogram buffer
    pub calculating_histogram: Arc<AtomicBool>, // Whether histogram calculation is in progress

    // Calibration state
    pub calibrating: Arc<AtomicBool>, // Whether calibration is in progress (for blinking button)
    // Cropped target-scan overlay (w, h, RGB bytes) from the last successful calibration.
    // When present, it replaces the calibration button as a visual "calibrated" indicator.
    pub calibration_overlay: Arc<ArcSwap<Option<(u32, u32, Vec<u8>)>>>,
    // scan_target's live_overlay, held over the frozen frame after calibration until the
    // user taps: (min_x, min_y, width, height, rgba_f32). Coords are in the full-res sensor
    // frame at 2x scale (as chameleon produces); RGBA linear 0..1. Composited in-place in
    // the fit-to-screen render loop. Cleared on the next touch.
    pub calibration_hold: Arc<ArcSwap<Option<(usize, usize, usize, usize, Vec<f32>)>>>,
    // Image counter snapshotted when calibration starts, so the feed freezes on that frame
    // while Chameleon computes (None = live).
    pub frozen_image_counter: Option<u64>,
    // Snapshot of the frozen slot's image data (avg+diff interleaved like image_buffer's
    // 2-slice layout) taken at calibration start. The camera thread keeps overwriting the
    // live slots, so we render from this copy to truly hold the frame. Empty = no freeze.
    pub frozen_image: Vec<u16>,
    // Tracks whether the full-screen hold was shown last frame, to force a clean repaint on
    // the frame it's dismissed.
    pub previous_hold_present: bool,

    pub arrow_buffers: ArrowBuffers,
    pub button_buffers: buttons::ButtonBuffers,
    pub calibration_button_buffer: buttons::CalibrationButtonBuffer,

    // Enables partial redraw tracking and clean regions below labels
    pub magic_counter: [u8; 3],
    pub left_label_buffer: Vec<u8>,
    pub left_label_x: usize,
    pub left_label_y: usize,
    pub left_label_end_x: usize,
    pub left_label_end_y: usize,
    pub right_label_buffer: Vec<u8>,
    pub right_label_x: usize,
    pub right_label_y: usize,
    pub right_label_end_x: usize,
    pub right_label_end_y: usize,
    // Label drawing positions for partial redraw
    pub left_label_draw_x: f32,
    pub left_label_draw_y: f32,
    pub right_label_draw_x: f32,
    pub right_label_draw_y: f32,
    pub label_text_height: f32,
    // Cached text for partial redraw optimization
    pub last_elapsed_text: String,
    pub last_remaining_text: String,
    pub time_base: TimeBase,
    // Cached slider coordinates for progress bar
    pub slider_start_x: f32,
    pub slider_start_y: f32,
    pub slider_end_x: f32,
    pub slider_end_y: f32,
    pub slider_thickness: f32,

    // Shared memory access - direct slices like camera integrator
    pub header: &'static mut [u64],   // Mutable for writing settings
    pub image_buffer: &'static [u16], // Read-only for reading image data
    pub magic_9_display: &'static mut [f32; 9],
    pub magic_9_display_gamma: &'static mut f32,
    pub magic_9_dng_xyz: &'static mut [f32; 9],
    pub magic_9_dng_xyz_gamma: &'static mut f32,

    // Keep SharedMemory alive (only for ASharedMemory instances)
    _shared_memory: Option<*mut SharedMemory>,

    // Previous image counter for new image detection
    pub previous_image_counter: u64,
    // Previous exposure start time for new exposure detection
    pub previous_exposure_start: u64,
    // Previous histogram counter for new histogram detection
    pub previous_histogram_counter: u64,

    // Counter text save/restore for partial redraws
    pub counter_buffers: Vec<Vec<u8>>, // 4 buffers for saved/frame/fps/format
    pub counter_areas: Vec<(usize, usize, usize, usize)>, // (x, y, end_x, end_y) for each
    // Dark-frame calibration: history of the noise stat sampled over the capture, for the live noise
    // graph on the stats screen. Appended (deduped on the frame count) as the capture runs so the user
    // can watch the ~1/sqrt(N) decrease flatten and stop when satisfied. Reset when a capture starts.
    pub cal_noise_history: Vec<f32>,
    pub cal_noise_last_frame: u64, // frame count of the last appended sample (dedup)
}

impl UserInterface {
    /// Create UserInterface from SharedMemory object - for proper IPC
    pub fn from_shared_memory_object(
        screen_width: u32,
        screen_height: u32,
        shared_memory: SharedMemory,
    ) -> Self {
        // Get the magic 9 array references before boxing (already 'static)
        let (magic_9_display, magic_9_display_gamma) = shared_memory.get_magic_9_display_slices();
        let (magic_9_dng_xyz, magic_9_dng_xyz_gamma) = shared_memory.get_magic_9_dng_xyz_slices();

        // Box the SharedMemory to keep it alive
        let shared_memory_box = Box::into_raw(Box::new(shared_memory));

        // Create from the raw pointer like before, but store the box to keep it alive
        let mut ui = Self::new(screen_width, screen_height, unsafe {
            (*shared_memory_box).as_slice().as_ptr() as *const u8
        });
        ui._shared_memory = Some(shared_memory_box);
        // Update the magic 9 references to use the ones from SharedMemory
        ui.magic_9_display = magic_9_display;
        ui.magic_9_display_gamma = magic_9_display_gamma;
        ui.magic_9_dng_xyz = magic_9_dng_xyz;
        ui.magic_9_dng_xyz_gamma = magic_9_dng_xyz_gamma;

        // Initialize magic_9_display if not already set (shared memory starts as zeros
        // which would produce black output). We bake in the Chameleon "rec2020-1" terminal
        // profile: the camera/LMS working RGB -> linear Rec.2020 matrix, decoded
        // (BLAKE3-validated) from rec2020-1.mag. The display surface is tagged BT.2020 so
        // Android does the final 2020 -> panel conversion. Row-major, R/G/B output rows.
        if ui.magic_9_display[0] == 0.0
            && ui.magic_9_display[4] == 0.0
            && ui.magic_9_display[8] == 0.0
        {
            ui.magic_9_display.copy_from_slice(&REC2020_DISPLAY_MATRIX);
            *ui.magic_9_display_gamma = 2.2;
            log::info!("Initialized magic_9_display to rec2020-1 terminal matrix");
        }

        ui
    }

    pub fn new(screen_width: u32, screen_height: u32, shared_memory_ptr: *const u8) -> Self {
        if crate::DEBUG {
            log::info!(
                "Creating UserInterface from SharedMemory: {}x{} screen, SharedMemory at 0x{:x}",
                screen_width,
                screen_height,
                shared_memory_ptr as u64
            );
        }

        // Create header slice - first part of SharedMemory
        let header =
            unsafe { std::slice::from_raw_parts_mut(shared_memory_ptr as *mut u64, IMAGE_START) };

        // Read camera info from SharedMemory header
        let sensor_width = header[SENSOR_WIDTH_IDX] as usize;
        let sensor_height = header[SENSOR_HEIGHT_IDX] as usize;
        let white_level = header[WHITE_LEVEL_IDX] as u16;
        let black_level = header[BLACK_LEVEL_IDX] as u16;
        let bayer_pattern = header[SENSOR_BAYER_PATTERN_IDX] as u32;
        let camera_facing = header[CAMERA_FACING_IDX] as i32;
        let min_iso = f64::from_bits(header[MIN_ISO_IDX]);
        let max_iso = f64::from_bits(header[MAX_ISO_IDX]);
        let min_shutter_ns = f64::from_bits(header[SHORTEST_SHUTTER_NS_IDX]);
        let max_shutter_ns = f64::from_bits(header[LONGEST_SHUTTER_NS_IDX]);
        let min_focus = f64::from_bits(header[MIN_FOCUS_IDX]);
        let sensor_orientation = header[SENSOR_ORIENTATION_IDX] as i32;

        // Create image buffer slice - starts after header
        let pixel_count = sensor_width * sensor_height;
        let image_buffer = unsafe {
            std::slice::from_raw_parts(
                shared_memory_ptr.add(IMAGE_START * 8) as *const u16,
                pixel_count * 8, // quad rolling buffer: 4 slots * 2 arrays per slot
            )
        };

        let screen_run = screen_width as usize;
        let screen_rise = screen_height as usize;
        let sensor_white_level = white_level;
        let sensor_black_level = black_level;
        let min_focus_distance = min_focus;
        let raw_average = image_buffer;
        let raw_difference = image_buffer;

        if crate::DEBUG {
            info!("Creating UserInterface");
            debug!("Screen dimensions: {}x{}", screen_run, screen_rise);
            debug!("Sensor dimensions: {}x{}", sensor_width, sensor_height);
            debug!(
                "Sensor levels: white={}, black={}, bayer_pattern={}",
                sensor_white_level, sensor_black_level, bayer_pattern
            );
            debug!(
                "Camera capabilities: ISO {:.0}-{:.0}, shutter {:.0}-{:.0}ns, focus {:.2}",
                min_iso, max_iso, min_shutter_ns, max_shutter_ns, min_focus_distance
            );
            debug!(
                "Camera orientation: {}°, facing: {}",
                sensor_orientation, camera_facing
            );
        }

        // Validate sensor levels
        if sensor_white_level <= sensor_black_level {
            error!(
                "Invalid sensor levels: white={}, black={}",
                sensor_white_level, sensor_black_level
            );
        }

        let dynamic_range = sensor_white_level.wrapping_sub(sensor_black_level);
        if crate::DEBUG {
            debug!("Sensor dynamic range: {} levels", dynamic_range);
        }

        if crate::DEBUG {
            debug!("Integration and raw image buffers created");
        }

        let initial_rotation = 0;

        // Calculate margins based on long/short dimensions, not X/Y
        let long_edge = screen_run.max(screen_rise);
        let short_edge = screen_run.min(screen_rise);

        let long_margin = long_edge as f32 * ui_constants::LONG_EDGE_MARGIN;
        let short_margin = short_edge as f32 * ui_constants::SHORT_EDGE_MARGIN;

        // Map margins to X/Y axes based on initial screen dimensions
        let x_margin = if screen_run > screen_rise {
            long_margin // X is the longer dimension
        } else {
            short_margin // X is the shorter dimension
        };

        let y_margin = if screen_rise > screen_run {
            long_margin // Y is the longer dimension
        } else {
            short_margin // Y is the shorter dimension
        };

        let screen_aspect = short_edge as f32 / long_edge as f32;

        if crate::DEBUG {
            debug!(
                "Screen layout: long_edge={}, short_edge={}, aspect={:.3}",
                long_edge, short_edge, screen_aspect
            );
            debug!(
                "Margins calculated: x={:.1}, y={:.1} (long={:.1}, short={:.1})",
                x_margin, y_margin, long_margin, short_margin
            );
        }

        // Calculate black level in 16-bit space
        let raw_black_level =
            (sensor_black_level as u32 * 65536 / sensor_white_level as u32) as u16;
        if crate::DEBUG {
            debug!(
                "Black level conversion: {} -> {} (16-bit)",
                sensor_black_level, raw_black_level
            );
        }

        if crate::DEBUG {
            debug!("Creating UI component buffers");
        }
        let arrow_buffers = ArrowBuffers::new_from_dimensions(
            screen_run,
            screen_rise,
            long_edge,
            short_edge,
            long_margin,
            short_margin,
        );

        let button_buffers = buttons::ButtonBuffers::new_from_dimensions(
            screen_run,
            screen_rise,
            long_edge,
            short_edge,
            long_margin,
            short_margin,
        );

        let calibration_button_buffer = buttons::CalibrationButtonBuffer::new(
            ((short_edge as f32 - short_margin * 2.) * CALIBRATION_BUTTON_SIZE) as usize,
        );

        let histogram_buffer_size = screen_run * screen_rise * 3;
        if crate::DEBUG {
            debug!("Histogram buffer size: {} bytes", histogram_buffer_size);
        }

        // Get magic 9 array references from header base address
        let magic_9_display =
            unsafe { &mut *(header.as_ptr().add(MAGIC_9_DISPLAY_IDX) as *mut [f32; 9]) };
        let magic_9_display_gamma =
            unsafe { &mut *(header.as_ptr().add(MAGIC_9_DISPLAY_IDX) as *mut f32).add(9) };
        let magic_9_dng_xyz =
            unsafe { &mut *(header.as_ptr().add(MAGIC_9_DNG_XYZ_IDX) as *mut [f32; 9]) };
        let magic_9_dng_xyz_gamma =
            unsafe { &mut *(header.as_ptr().add(MAGIC_9_DNG_XYZ_IDX) as *mut f32).add(9) };

        let ui = Self {
            screen_run,
            screen_rise,
            screen_aspect,
            sensor_x_size: sensor_width,
            sensor_y_size: sensor_height,
            x_margin,
            y_margin,
            sensor_orientation,
            camera_facing,
            device_rotation: initial_rotation,
            sensor_white_level,
            sensor_black_level,
            bayer_pattern,
            raw_average,
            raw_difference,
            raw_black_level,
            display_gain: 1.,
            min_iso,
            max_iso,
            min_shutter_ns,
            max_shutter_ns,
            min_focus_distance,
            exposure_time_ms: f64::from_bits(header[EXPOSURE_TIME_MS_IDX]) as u64,
            // Initialize sliders from current SharedMemory values
            exposure_time_slider: {
                let current_exposure_ms = f64::from_bits(header[EXPOSURE_TIME_MS_IDX]);
                if current_exposure_ms > 0.0 {
                    // Auto-select best timescale and calculate slider position
                    let time_base = if current_exposure_ms < 60_000.0 {
                        TimeBase::Minute
                    } else if current_exposure_ms < 3_600_000.0 {
                        TimeBase::Hour
                    } else if current_exposure_ms < 86_400_000.0 {
                        TimeBase::Day
                    } else if current_exposure_ms < 2_592_000_000.0 {
                        TimeBase::Month
                    } else {
                        TimeBase::Year
                    };
                    (current_exposure_ms / time_base.duration_ms())
                        .sqrt()
                        .min(1.0)
                } else {
                    0.0 // No exposure set yet
                }
            },
            iso_slider: {
                let current_iso = f64::from_bits(header[ISO_IDX]);
                (current_iso / min_iso).ln() / (max_iso / min_iso).ln()
            },
            shutter_slider: {
                let current_shutter_ns = f64::from_bits(header[SHUTTER_NS_IDX]);
                (current_shutter_ns / min_shutter_ns).ln() / (max_shutter_ns / min_shutter_ns).ln()
            },
            focus_slider: {
                let current_focus = f64::from_bits(header[FOCUS_IDX]);
                if min_focus_distance > 0.0 {
                    current_focus / min_focus_distance
                } else {
                    0.0 // Infinity focus
                }
            },
            gain_slider: 0., // UI-only display gain, starts at 0
            active_track: None,
            controls_visible: true,
            view_1to1: false,
            pan_offset_x: 0.,
            pan_offset_y: 0.,
            touch_start_x: 0.,
            touch_start_y: 0.,
            last_touch_x: 0.,
            last_touch_y: 0.,
            is_dragging: false,
            touch_is_dead: false,
            last_touch_valid: false,
            text_renderer: TextRenderer::new(),
            pressed_arrow: None,
            active_button: None,
            histogram_visible: false,
            histogram_buffer: Arc::new(ArcSwap::from_pointee(None)),
            calculating_histogram: Arc::new(AtomicBool::new(false)),
            calibrating: Arc::new(AtomicBool::new(false)),
            calibration_overlay: Arc::new(ArcSwap::from_pointee(None)),
            calibration_hold: Arc::new(ArcSwap::from_pointee(None)),
            frozen_image_counter: None,
            frozen_image: Vec::new(),
            previous_hold_present: false,
            arrow_buffers,
            button_buffers,
            calibration_button_buffer,
            magic_counter: [0x05, 0x03, 0x16],
            left_label_buffer: Vec::new(),
            left_label_x: 0,
            left_label_y: 0,
            left_label_end_x: 0,
            left_label_end_y: 0,
            right_label_buffer: Vec::new(),
            right_label_x: 0,
            right_label_y: 0,
            right_label_end_x: 0,
            right_label_end_y: 0,
            left_label_draw_x: 0.0,
            left_label_draw_y: 0.0,
            right_label_draw_x: 0.0,
            right_label_draw_y: 0.0,
            label_text_height: 0.0,
            last_elapsed_text: String::new(),
            last_remaining_text: String::new(),
            time_base: {
                // Auto-select best timescale based on current exposure time
                let current_exposure_ms = f64::from_bits(header[EXPOSURE_TIME_MS_IDX]);
                if current_exposure_ms > 0.0 {
                    if current_exposure_ms < 60_000.0 {
                        TimeBase::Minute
                    } else if current_exposure_ms < 3_600_000.0 {
                        TimeBase::Hour
                    } else if current_exposure_ms < 86_400_000.0 {
                        TimeBase::Day
                    } else if current_exposure_ms < 2_592_000_000.0 {
                        TimeBase::Month
                    } else {
                        TimeBase::Year
                    }
                } else {
                    TimeBase::Minute // Default for no exposure
                }
            },
            slider_start_x: 0.0,
            slider_start_y: 0.0,
            slider_end_x: 0.0,
            slider_end_y: 0.0,
            slider_thickness: 0.0,
            header,
            image_buffer,
            magic_9_display,
            magic_9_display_gamma,
            magic_9_dng_xyz,
            magic_9_dng_xyz_gamma,
            _shared_memory: None,
            previous_image_counter: 0,
            previous_exposure_start: 0,
            previous_histogram_counter: 0,
            counter_buffers: vec![Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            counter_areas: vec![(0, 0, 0, 0), (0, 0, 0, 0), (0, 0, 0, 0), (0, 0, 0, 0)],
            cal_noise_history: Vec::new(),
            cal_noise_last_frame: 0,
        };

        if crate::DEBUG {
            info!("UserInterface created successfully");
            debug!("Initial slider values: gain={:.3}, focus={:.3}, shutter={:.3}, iso={:.3}, exposure={:.3}", 
                   ui.gain_slider, ui.focus_slider, ui.shutter_slider, ui.iso_slider, ui.exposure_time_slider);
            debug!(
                "Magic counter initialized: {:02X}{:02X}{:02X}",
                ui.magic_counter[0], ui.magic_counter[1], ui.magic_counter[2]
            );
        }

        ui
    }

    pub fn get_button_info(&self, button_index: usize, is_pressed: bool) -> (String, [u8; 3]) {
        let text = match button_index {
            0 => {
                if self.histogram_visible {
                    "LIVE".to_string()
                } else {
                    "HISTOGRAM".to_string()
                }
            }
            1 => self.time_base.clone().as_str().to_string(),
            2 => {
                let current_mode = RawMode::from(self.header[CURRENT_MODE_IDX] as u8);
                let mode_name = match current_mode {
                    RawMode::Average => "AVERAGE",
                    RawMode::Difference => "DIFFERENCE",
                    RawMode::Motion => "MOTION",
                };
                mode_name.to_string()
            }
            3 => "EXIT".to_string(),
            _ => "".to_string(),
        };

        let colour = if button_index < ui_constants::BUTTON_COLOURS.len() {
            let (r, g, b) = ui_constants::BUTTON_COLOURS[button_index];
            if is_pressed {
                // Pressed color - multiply by 1.5 like the button rendering
                [
                    (r * 1.5 * 256.) as u8,
                    (g * 1.5 * 256.) as u8,
                    (b * 1.5 * 256.) as u8,
                ]
            } else {
                [(r * 256.) as u8, (g * 256.) as u8, (b * 256.) as u8]
            }
        } else {
            [255, 255, 255]
        };

        (text, colour)
    }

    pub fn handle_button_press(&mut self, button_index: usize) {
        match button_index {
            0 => self.histogram_visible = !self.histogram_visible,
            1 => {
                self.time_base = self.time_base.clone().next();
                // Update slider position to maintain same actual duration on new scale
                // Update exposure time slider based on current exposure time
                self.exposure_time_slider = (self.exposure_time_ms as f64
                    / self.time_base.duration_ms())
                .sqrt()
                .min(1.0);
            }
            2 => {
                let current_mode = RawMode::from(self.header[CURRENT_MODE_IDX] as u8);
                let next_mode = match current_mode {
                    RawMode::Average => RawMode::Difference,
                    RawMode::Difference => RawMode::Motion,
                    RawMode::Motion => RawMode::Average,
                };
                self.header[CURRENT_MODE_IDX] = next_mode as u64;
            }
            3 => {
                // Exit - clear continuous save and signal camera thread then kill the UserInterface process
                if crate::DEBUG {
                    log::info!("Exit button pressed - clearing continuous save, signaling camera thread and killing UserInterface process");
                }

                // Clear continuous save bit so camera thread will die
                self.header[FLAGS_IDX] &= !CONTINUOUS_SAVE_BIT;
                self.header[HEARTBEAT_SECS_IDX] -= 256;

                unsafe {
                    libc::exit(0);
                }
            }
            4 => {
                // Don't start another calibration if one is already running
                if self.calibrating.load(std::sync::atomic::Ordering::Relaxed) {
                    log::info!("Calibration already in progress, ignoring button press");
                    return;
                }

                log::info!("Starting Chameleon colour calibration");

                // Set calibrating flag for blinking button
                self.calibrating
                    .store(true, std::sync::atomic::Ordering::Relaxed);

                // Get current image slot from SharedMemory header
                let image_counter = self.header[IMAGE_COUNTER_IDX];
                // Freeze the feed on the exact frame we calibrate from while Chameleon runs.
                self.frozen_image_counter = Some(image_counter);
                let current_slot = (image_counter & 3) as usize;
                let pixel_count = self.sensor_x_size * self.sensor_y_size;
                // Snapshot the frozen slot's avg+diff data so the camera thread overwriting
                // the live slots can't change what we display while held.
                let slot_start = (current_slot * 2) * pixel_count;
                self.frozen_image =
                    self.image_buffer[slot_start..slot_start + 2 * pixel_count].to_vec();

                // Calculate quad rolling buffer offsets: [slot0_avg, slot0_diff, slot1_avg, slot1_diff, ...]
                let avg_offset = (current_slot * 2) * pixel_count;

                let raw_average = &self.image_buffer[avg_offset..avg_offset + pixel_count];

                // Quad-Bayer (max-res RAW10) frames are a 4x4 Tetracell CFA; chameleon's debayer only understands standard 2x2 Bayer. Pre-bin each 2x2 same-colour cluster into one pixel, yielding a half-size standard 2x2 Bayer frame with the same base pattern, before handing it to scan_target. Calibration only needs accurate colour, not 50MP resolution.
                let is_quad = self.header[crate::shared_memory::QUAD_BAYER_IDX] != 0;
                let (img, width, height) = if is_quad {
                    let (ow, oh, binned) = crate::debayer::quad::quad_to_standard_bayer(
                        raw_average,
                        self.sensor_x_size,
                        self.sensor_y_size,
                    );
                    (binned, ow, oh)
                } else {
                    (raw_average.to_vec(), self.sensor_x_size, self.sensor_y_size)
                };

                // Capture values needed for the background thread
                let bayer_pattern = self.bayer_pattern;
                let black_level = self.raw_black_level;
                let magic_9 = *self.magic_9_display;
                let calibrating_flag = self.calibrating.clone();
                let calibration_overlay = self.calibration_overlay.clone();
                let calibration_hold = self.calibration_hold.clone();

                // Pointers to shared memory for writing calibration results back (as usize for Send)
                let magic_9_display_addr = self.magic_9_display.as_mut_ptr() as usize;
                let magic_9_display_gamma_addr = (self.magic_9_display_gamma as *mut f32) as usize;
                // XYZ matrix (for RGB exports) and the DNG magic9inv bytes live in shared
                // memory too; capture their addresses so the worker can write the results.
                let magic_9_dng_xyz_addr = self.magic_9_dng_xyz.as_mut_ptr() as usize;
                let magic_9_dng_xyz_gamma_addr = (self.magic_9_dng_xyz_gamma as *mut f32) as usize;
                let magic_9_inv_addr =
                    unsafe { self.header.as_mut_ptr().add(MAGIC_9_INV_IDX) as *mut u8 } as usize;

                // Spawn calibration work on background thread to avoid ANR
                // (sync_profiles makes blocking HTTP requests)
                std::thread::spawn(move || {
                    log::info!("Calibration thread started");
                    let mut img = img;

                    // Build DNG header
                    let mut raw_info = RawInfo {
                        make: "Android".to_owned(),
                        makeoffset: 0,
                        makelen: 0,
                        model: "Lumis".to_owned(),
                        modeloffset: 0,
                        modellen: 0,
                        width,
                        height,
                        bitdepth: 16,
                        bitdepthold: 0,
                        rgb: false,
                        cfa: match bayer_pattern {
                            0 => vec![0, 1, 1, 2], // RGGB
                            1 => vec![1, 0, 2, 1], // GRBG
                            2 => vec![1, 2, 0, 1], // GBRG
                            3 => vec![2, 1, 1, 0], // BGGR
                            _ => vec![0, 1, 1, 2], // Default to RGGB
                        },
                        cfaw: 2,
                        cfah: 2,
                        black: black_level as f32,
                        blackoffset: 0,
                        blackcount: 0,
                        blacktype: 0,
                        white: 65535.,
                        orientation: 0,
                        compression: false,
                        cam2terminal9: magic_9,
                        magic9inv: [
                            1, 0, 0, 0, 1, 0, 0, 0, //
                            0, 0, 0, 0, 1, 0, 0, 0, //
                            0, 0, 0, 0, 1, 0, 0, 0, //
                            //
                            0, 0, 0, 0, 1, 0, 0, 0, //
                            1, 0, 0, 0, 1, 0, 0, 0, //
                            0, 0, 0, 0, 1, 0, 0, 0, //
                            //
                            0, 0, 0, 0, 1, 0, 0, 0, //
                            0, 0, 0, 0, 1, 0, 0, 0, //
                            1, 0, 0, 0, 1, 0, 0, 0, //
                        ],
                        magicoffset: 0,
                        profileoffset: 0,
                        curveoffset: 0,
                        imagedataoffset: 0,
                        ifdoffset: 0,
                        duck: false,
                        save_scan: false,
                    };
                    let (success, settings) = get_settings();
                    if !success {
                        log::info!("Unable to sync profiles and settings!");
                    } else {
                        // The scan is produced at the defished image's native size (896x768,
                        // 7:6) - overlay_readout uses scan_target's own patchdim now, not
                        // settings.overlay_width. Our compositor scales + letterboxes it into
                        // the square button.
                        match scan_target(
                            &mut ImageData::U16Data(&mut img),
                            &mut raw_info,
                            &settings,
                            true,
                        ) {
                            Some((
                                overlaywidth,
                                overlayheight,
                                overlayimage,
                                _report,
                                _warning,
                                live_overlay,
                            )) => {
                                // Write the calibration results to shared memory:
                                // - cam2terminal9 (Rec.2020) -> magic_9_display for live preview.
                                // - magic9inv (camera->XYZ ColorMatrix1) -> for the DNG.
                                // - XYZ export matrix -> magic_9_dng_xyz. We don't yet have a
                                //   separate XYZ terminal profile, so seed it from cam2terminal9
                                //   for now (TODO: real XYZ profile). RGB exports use this.
                                unsafe {
                                    let magic9_ptr = magic_9_display_addr as *mut f32;
                                    let magic9_slice =
                                        std::slice::from_raw_parts_mut(magic9_ptr, 9);
                                    magic9_slice.copy_from_slice(&raw_info.cam2terminal9);
                                    *(magic_9_display_gamma_addr as *mut f32) =
                                        settings.terminal9[9];

                                    let xyz_ptr = magic_9_dng_xyz_addr as *mut f32;
                                    let xyz_slice = std::slice::from_raw_parts_mut(xyz_ptr, 9);
                                    xyz_slice.copy_from_slice(&raw_info.cam2terminal9);
                                    *(magic_9_dng_xyz_gamma_addr as *mut f32) =
                                        settings.terminal9[9];

                                    let inv_ptr = magic_9_inv_addr as *mut u8;
                                    std::ptr::copy_nonoverlapping(
                                        raw_info.magic9inv.as_ptr(),
                                        inv_ptr,
                                        raw_info.magic9inv.len(),
                                    );
                                }

                                // Hold the in-place live overlay over the frozen frame until tap.
                                if let Some(lo) = live_overlay {
                                    calibration_hold.store(Arc::new(Some(lo)));
                                }
                                // Keep the cropped scan in the calibration button as the
                                // persistent "calibrated" indicator.
                                calibration_overlay.store(Arc::new(Some((
                                    overlaywidth,
                                    overlayheight,
                                    overlayimage,
                                ))));
                                log::info!(
                                    "Wrote display matrix to shared memory: {:?}, gamma: {}",
                                    raw_info.cam2terminal9,
                                    settings.terminal9[9]
                                );

                                let data = encode_settings(&settings);
                                let mut settingsfile = verichrome_dir();
                                settingsfile.push("settings.cfg");
                                write_settings(data, &settingsfile);
                                log::info!("Calibration completed successfully");
                            }
                            None => {
                                log::info!("Calibration scan_target returned None");
                            }
                        }
                    }
                    // Clear calibrating flag when done
                    calibrating_flag.store(false, std::sync::atomic::Ordering::Relaxed);
                    log::info!("Calibration thread finished");
                });
            }
            _ => {}
        }
    }

    pub fn format_time(&self, ms: u64) -> String {
        fn format_3_sig_figs(value: f64, unit: &str) -> String {
            if value == 0.0 {
                return format!("0{}", unit);
            }

            let magnitude = value.abs().log10().floor() as i32;
            let precision = 2 - magnitude;

            if precision >= 0 {
                format!("{:.prec$}{}", value, unit, prec = precision as usize)
            } else {
                format!("{:.0}{}", value, unit)
            }
        }

        let result = match ms {
            0 => "0s".to_string(),
            1..=999 => {
                let seconds = ms as f64 / 1000.0;
                format_3_sig_figs(seconds, "s")
            }
            1000..=59999 => {
                let seconds = ms as f64 / 1000.0;
                format_3_sig_figs(seconds, "s")
            }
            60000..=3599999 => {
                let minutes = ms as f64 / 60000.0;
                format_3_sig_figs(minutes, "m")
            }
            3600000..=86399999 => {
                let hours = ms as f64 / 3600000.0;
                format_3_sig_figs(hours, "h")
            }
            _ => {
                let days = ms as f64 / 86400000.0;
                format_3_sig_figs(days, "d")
            }
        };

        if crate::DEBUG {
            trace!("Time formatting: {}ms -> '{}'", ms, result);
        }
        result
    }

    pub fn is_fat(&self) -> bool {
        let result = match self.device_rotation {
            0 | 180 => self.screen_run > self.screen_rise,
            _ => self.screen_run < self.screen_rise,
        };
        result
    }

    /// Set flag in SharedMemory for Camera thread communication
    pub fn set_shared_memory_flag(&mut self, flag_bit: u64) {
        self.header[FLAGS_IDX] |= flag_bit;
        if crate::DEBUG {
            log::debug!("Set SharedMemory flag bit: 0x{:x}", flag_bit);
        }
    }

    /// Handle input and draw screen - called at display refresh rate
    pub fn handle_input_and_draw(
        &mut self,
        window: &ndk::native_window::NativeWindow,
        touch_x: f32,
        touch_y: f32,
        gravity_x: f32,
        gravity_y: f32,
        _gravity_z: f32,
    ) {
        let mut draw = false;
        let unix_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        self.header[HEARTBEAT_SECS_IDX] = unix_time.as_secs();
        self.header[HEARTBEAT_NANOS_IDX] = unix_time.subsec_nanos() as u64;
        let new_rotation = if gravity_y.abs() > 7.0 && gravity_y > 0.0 {
            0 // Normal portrait
        } else if gravity_x.abs() > 7.0 && gravity_x < 0.0 {
            90 // Landscape (rotated left)
        } else if gravity_y.abs() > 7.0 && gravity_y < 0.0 {
            180 // Upside down
        } else if gravity_x.abs() > 7.0 && gravity_x > 0.0 {
            270 // Landscape (rotated right)
        } else {
            self.device_rotation // Keep current if below threshold
        };

        // Update device rotation if changed
        if new_rotation != self.device_rotation {
            if crate::DEBUG {
                log::info!(
                    "Device rotation changed: {} -> {} (gravity: x={:.2}, y={:.2})",
                    self.device_rotation,
                    new_rotation,
                    gravity_x,
                    gravity_y
                );
            }
            self.device_rotation = new_rotation;
            draw = true;
        }
        let current_touch_valid = !touch_x.is_nan();

        // A fresh touch while the full-screen calibration scan is held dismisses it (and
        // unfreezes the feed). Per design the same touch still passes through to normal
        // handling below, so e.g. tapping a control both dismisses and acts.
        if current_touch_valid && !self.last_touch_valid && self.calibration_hold.load().is_some() {
            self.calibration_hold.store(std::sync::Arc::new(None));
            self.frozen_image_counter = None;
            self.frozen_image = Vec::new();
            draw = true;
        }

        draw |= match (current_touch_valid, self.last_touch_valid) {
            (false, false) => false,
            (true, false) => handle_touch(self, TouchAction::Down, touch_x, touch_y, 1., 0),
            (true, true) => handle_touch(self, TouchAction::Hold, touch_x, touch_y, 1., 0),
            (false, true) => handle_touch(
                self,
                TouchAction::Up,
                self.last_touch_x,
                self.last_touch_y,
                1.,
                0,
            ),
        };

        self.last_touch_x = touch_x;
        self.last_touch_y = touch_y;
        self.last_touch_valid = current_touch_valid;

        draw_screen(self, window, draw);
    }
}
