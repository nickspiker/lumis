# Lumis Camera

**Full Manual RAW Camera for Android**

> **⚠️ WARNING: No Auto Anything!**  
> You want auto-focus? Auto-exposure? Auto white balance? **Look somewhere else!** This is a fully manual RAW camera app for those who want complete control over every photon hitting the sensor.

## What is Lumis?

Lumis is an experimental Android camera app that provides **direct manual control** over camera sensors with zero automatic processing. It captures pure RAW sensor data and lets you control every aspect of image capture manually.

### Key Features

- **100% Manual Control** - No auto, period.
- **Pure RAW Capture** - Direct sensor readout with zero processing
- **Long Exposure Support** - Accumulate frames over seconds, minutes, or years, yes, years!
- **Computational Modes**:
  - **Long Exposure** - Traditional frame accumulation and averaging
  - **Difference Sum** - Captures the overall difference
  - **Motion Extraction** - Isolates movement in the scene
- **Manual Controls**:
  - ISO (sensor gain)
  - Shutter speed (per frame)
  - Focus distance
  - Display gain (preview brightness, does not affect the final RAW image)
  - Exposure time

## Requirements

- Android device with Camera2 API support
- Camera with RAW support
- Android 6.0 (API 23) or higher
- Patience and willingness to learn manual photography? It's pretty fun once you get it figured out.

## How to Use

### Main Menu
When you launch Lumis, you'll see a list of available cameras that support RAW capture. Each camera shows:
- Resolution
- Sensor type (front/back)
- Focal length range
- Various other camera metrics

Tap a camera to start capturing.

### Camera Interface

The camera view is divided into three areas:

1. **Image Preview** (Top 60%)
   - Tap to toggle clipping indicators/controls
   - Drag to zoom in at 1:1 pixel view
   - Tap while zoomed to return to fit view
   - Shot mode, timescale, exit and 

2. **Control Sliders** (Middle)
   - **Timer**: Integration duration (0-60s/min/hr)
   - **ISO**: Sensor gain
   - **Shutter**: Per-frame exposure time
   - **Focus**: Manual focus distance
   - **Gain**: Display brightness adjustment

3. **Buttons** (Bottom)
   - **Mode**: Switch between Long Exp/Diff Sum/Motion Ext
   - **Timeframe**: Change timer scale (Seconds/Minutes/Hours/Days/Years)
   - **Histogram**: Toggle histogram view
   - **Exit**: Return to camera selection

### Capture Process

1. **Set your parameters**:
   - Adjust ISO for sensor gain
   - Set shutter speed for per-frame exposure
   - Set focus distance manually
   - Choose integration time with timer slider

2. **Start capture**:
   - Frames begin accumulating automatically
   - Green progress bar shows integration progress
   - Frame counter displays captured frames
   - Shot counter displays number of saved images

3. **During capture**:
   - Adjust display gain to make preview/focus easy
   - Change parameters (takes effect on next integration, does not affect current exposure)
   - Monitor for clipping with tap
   - Tap also shows/hides controls

4. **Save image**:
   - Press volume down button to save current integration
   - Press volumu up to start continuous save
   - Press volume down while continuous save is actcive to stop continuous save, otherwise save single image
   - Images saved as 16-bit linear TIFFs and DNG

### Exposure Modes

- **Long Exposure**: Accumulates all frames together. Great for:
  - Night photography
  - Star trails
  - Light painting
  - Reducing noise thru averaging

- **Difference Sum**: Accumulates frame-to-frame differences. Captures:
  - Motion trails
  - Changes in the scene
  - Moving subjects against static backgrounds

- **Motion Extraction**: Isolates only moving elements. Perfect for:
  - Removing static elements
  - Creating motion studies
  - Experimental effects

## Technical Details

### Architecture
- **Frontend**: All Rust, only Kotlin where required by Android
- **Backend**: 100% Rust for obvious reasons
- **Rendering**: Completely custom and complete imaging pipeline

### Processing Pipeline
1. RAW sensor data
2. 2x2 Bayer pattern binning for fit on screen preview
2. AMaZE (Aliasing Minimization and Zipper Elimination) for zoomed in preview
3. Frame accumulation in 32-bit buffers
4. Linear to display conversion for accurate colour (if profiled)
5. No noise reduction, sharpening, or enhancement.  Just untouched RAW data!

### Supported Formats
- **Input**: RAW_SENSOR format only!
- **Preview**: RGBA8888 with colourspace conversion
- **Output**: VSF (Versatile Storage Format), DNG (Digital Negative) and TIFF (Tagged Image File Format)

## Why Manual Only?

Modern camera apps hide the beauty of direct sensor control behind layers of automation. Lumis strips all that away, giving you:

- **Learning**: Understand how digital imaging really works
- **Control**: Every pixel is your decision
- **Creativity**: Techniques impossible with auto modes
- **Quality**: No unwanted processing artifacts

Built with:
- Android Camera2 API
- Rust for performance
- Cosmic Text for rendering
- Pure stubbornness against auto modes

---

**Remember**: If you want easy, use your phone's default camera. If you want control, welcome to Lumis! 📸
