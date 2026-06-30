# Lumis — Complete Feature List

Lumis is a manual RAW camera for Android with a Rust core and a spectral colour-calibration engine (chameleon). It captures true 16-bit RAW, integrates frames to push effective ISO below 1, and renders scene-accurate colour from measured spectral reflectance — all on-device.

## Headline capabilities

- **Effective ISO below 1.** Stacking N frames gathers N times the light, so effective exposure = N × per-frame shutter and effective ISO = per-frame ISO / N — driven well below 1 with enough frames, and recorded as the composite exposure in EXIF.
- **Extreme long integration up to whole years.** A Minute/Hour/Day/Month/Year timebase lets the exposure slider span fractions of a second to an entire year, building dynamic range through integration instead of one giant exposure.
- **Direct Scene Referred™ (DSR™) spectral colour.** Every calibration patch carries a full measured spectral reflectance curve integrated against cone/CMF response, so colours render as they'd appear under a chosen reference illuminant — a spectral relative IDT, not a 3-point white balance. *(Direct Scene Referred™ is a trademark of Achtel Pty Limited.)*
- **True 16-bit raw and pipeline.** 10-bit sensor samples accumulate in 32-bit and rescale through a full 16-bit/f32 pipeline; DNG raw is genuinely 16-bit-per-pixel and TIFF exports are 16-bit-per-channel, with all colour maths in linear f32. (JPEG and JPEG XL exports are 8-bit.)
- **Photo-finish slitscan.** A scrolling time-strip mode where the vertical axis is time, captured into a 2:1 ring buffer with run/pause and a wrap-around circular time-axis inspector.
- **On-device dark-frame + colour calibration.** Capture your own per-sensor bias/dark calibration (saved as a self-verifying, BLAKE3-checksummed file with on-device read-back verification), and scan a Verichrome colour target to rewrite the live preview and DNG colour matrices in place.
- **Quad-Bayer (Tetracell) 50MP max-res.** Full non-binned sensor readout supported end-to-end, tagged with its real 4×4 CFA and demosaiced with a purpose-built quad algorithm validated to ~33 dB on edges / ~35 dB on natural detail (rel-PSNR).
- **Dual-process, zero-copy architecture.** A dedicated `:camera` process owns the sensor and shares one ashmem buffer with the UI process with no per-frame copying, guarded by a mutual self-kill watchdog.
- **Rec.2020 wide-gamut throughout.** Live preview renders on a true BT.2020 display surface; every RGB export is Rec.2020-primaried and ICC/codestream-tagged.
- **Silent shooting.** A self-managed Telecom "call" suppresses notification sound and vibration while you shoot, without touching media volume, and releases the instant the app closes.

---

## Capture & Integration

- **Average mode (long-exposure frame stack).** Sum many frames into one clean long exposure, averaged and rescaled to 16-bit. The base integration model all other modes derive from.
- **Difference mode (frame-to-frame change).** Accumulate the absolute difference between consecutive frames over the exposure, producing a zero-centred change image.
- **Motion mode (difference / average).** Normalise frame-to-frame change against local scene brightness to reveal motion, saving both planes and computing motion per pixel at encode time.
- **Slitscan mode (photo-finish time-strip).** Take one Bayer-period band from each frame's centre and stack it into a 2:1 ring buffer where the vertical axis is time; each column is integrated like an Average exposure, keeping the strip a valid demosaicable raw.
- **Low effective ISO / extended dynamic range via integration.** N frames = N × the light, so effective ISO drops by N (below 1 for large stacks) and accumulated signal exceeds any single frame's range.
- **16-bit accumulation from a 10-bit sensor.** Per-frame 0–1023 values sum in 32-bit accumulators and rescale to a full 16-bit result clamped to 65535.
- **Minute→Year timebase drives real integration length.** The exposure slider's value squared times the selected timebase duration sets the actual capture duration the integrator uses to complete each exposure (and each slitscan column), so the Minute/Hour/Day/Month/Year selector sets genuine integration time.
- **Rolling 4-slot interleaved buffer.** A completed capture stays live for save/inspect while the next exposure integrates, so saving a long exposure never waits for the next frame.
- **Slitscan ring buffer with continuous roll.** A fixed width × 2·width strip whose write-head advances one Bayer period per column and wraps, overwriting the oldest slice with no data shuffling; assembled in chronological order on save with CFA phase preserved.
- **Slitscan run/pause via Bluetooth remote.** Freeze the time-strip to inspect, zoom, or save, then resume from exactly where it stopped without clearing the captured strip.
- **Slitscan survives mode switching.** The strip lives in its own buffer region, so leaving slitscan for another mode and returning keeps it intact.
- **Bluetooth shutter force-completion.** A remote shutter press finishes the current exposure immediately and publishes the partial stack (repurposed to toggle run/pause in slitscan).
- **Continuous-save (timelapse/burst) mode.** Hold the trigger to keep saving frames; this also exempts the camera process from the heartbeat auto-kill so long captures aren't interrupted.
- **Frame-decoupled settings (30Hz poll).** ISO, shutter, focus, the shutter, and saves are read straight from shared memory and applied in ~33ms instead of waiting a full (up to 16s) frame.
- **Captured per-frame exposure feedback.** Each frame records the ISO, shutter, and focus the HAL actually applied, so metadata reflects reality rather than the request.
- **FPS and exposure-progress telemetry.** Live frame rate, exposure-start time, and frame counters are published so the UI can show a long-exposure progress bar and live elapsed/remaining timers.

## Sensor & Camera Control

- **16-bit RAW_SENSOR capture (binned mode).** Captures unprocessed 16-bit RAW straight from the sensor in standard binned mode with no depacking needed.
- **RAW10 (MIPI CSI-2) capture + on-device depacking.** Captures 10-bit MIPI-packed RAW10 (4 px / 5 bytes, row-strided) and unpacks it losslessly on the camera thread so the rest of the pipeline is format-agnostic.
- **Quad-Bayer / Tetracell maximum-resolution mode.** Unlocks the full non-binned readout (e.g. 50MP vs 12.5MP on a Pixel main lens), setting the required max-res pixel mode on every request.
- **Manual ISO with real sensor range.** Bounded by the sensor's reported sensitivity range, applied on a full-manual (CONTROL_MODE_OFF) request.
- **Manual shutter / long exposure with real sensor range.** From the sensor's shortest exposure up to its multi-second maximum, verified that a 1s request yields a 1s result on the next frame.
- **Auto-exposure scene seed (one-shot AE warm-up).** On open the camera meters the scene once with auto-exposure, then locks to manual seeded with sensible values — so sliders always open at usable settings on any device. Focus stays manual throughout.
- **Multi-lens / multi-sensor enumeration.** Lists every real physical RAW-capable sensor — wide, ultrawide, telephoto, front — surfacing hidden physical sub-cameras behind logical cameras instead of duplicating the wrapper.
- **Physical sub-camera targeting.** Routes the RAW stream to the chosen physical sensor behind a logical camera so hidden ultrawide/telephoto sensors are openable.
- **Per-lens mode grouping + resolution sub-picker.** Groups each physical lens's binned, max-res, and cropped readouts under one lens so you can drill in and pick a resolution.
- **Cropped / digital-crop readout detection.** Flags RAW modes that image only a sub-region of the sensor by comparing aspect ratios to the active array.
- **White / black level handling.** Reads each sensor's true white and black levels and carries them into the RAW output and calibration scaling.
- **Bayer / colour-filter pattern detection.** Detects the CFA arrangement and applies the matching demosaic (RCD for 2×2, quad for 4×4).
- **Effective active-array dimensions for accurate FOV.** Scales physical sensor size by active-array/pixel-array so field-of-view and 35mm-equivalent focal length are correct on every lens, including telephoto.
- **OIS, hardware level, and sensor orientation reporting.** Enumerates optical stabilization availability, Camera2 hardware level, and sensor mount angle per camera.
- **Max-res RAW size discovery with Pixel workaround.** Synthesizes the max-res RAW size from the pixel-array metadata on Pixel devices whose framework advertises RAW10 but returns no size.
- **Robust device-quirk-tolerant enumeration.** Packs the full camera catalogue as a delimited float array and decodes it defensively, bounds-checking every field with safe defaults for missing ones.

## Colour Science (chameleon)

- **Direct Scene Referred™ colour transform.** Solves a camera-native→reference matrix from measured spectral reflectance under a chosen illuminant + observer — a spectral relative IDT, not a von-Kries white-balance gain. *(Direct Scene Referred™ is a trademark of Achtel Pty Limited.)*
- **Spectral reflectance reconstruction per patch.** Each patch carries its full multi-band reflectance curve, integrated against LMS cone fundamentals and the RGB/CMF response to synthesise both destination and reference colour.
- **Magic-9 colour matrix solve.** Coordinate-descent least-squares fit producing the final camera→reference 3×3 matrix, chained through indicator-fit, CMF, and profile matrices.
- **Colour-target auto-scan & geometric recovery.** Finds and reads the target even rotated, flipped, or lens-distorted, via FFT bullseye search, projective transform, a 16-dot alignment mesh, and Lagrange-interpolated de-fishing onto a 7×6 patch grid.
- **Round-code target ID with error correction.** A 126-bit ring code decoded with Reed-Solomon correction plus XOR/PRNG descramble and checksum recovers the target type and 64-bit serial.
- **Per-target encrypted, device-bound calibration sync.** Each target's factory spectral calibration is fetched over HTTP, re-encrypted to a device-local BLAKE3-derived key, signature-verified, and cached.
- **Overexposure detection / scan rejection.** Rejects the scan if any patch channel exceeds 50% of full scale (median, not max, so hot pixels don't false-trigger), surfacing the reason to the UI.
- **Host-readable scan-failure reason.** Any scan rejection sets a global last-scan-error string surfaced to the host UI, so every colour-cal failure (not just overexposure) carries a human-readable reason on-device.
- **Gamma-linearity check.** Least-squares fits gamma from known-reflectance grey patches and warns (colour-coded) if the data isn't linear.
- **IR leak measurement.** Reports per-channel infrared contamination (R/G/B %) from dedicated IR-black/IR-white indicator patches.
- **Relative scene UV measurement.** Reports how much ultraviolet is present in the scene light, relative to the target, from a UV indicator patch.
- **Target life / fugitive-dye wear indicator.** A fading indicator patch yields a 0–100% "Target life" reading that tells you when to replace the target.
- **DNG-embedded colour matrix (rational encoding).** Writes the inverse profile matrix into the DNG as exact i32 rational pairs, refusing to embed an "extreme" matrix; profile named "Verichrome scene-relative IDT."
- **Display/terminal matrix + gamma for live preview.** A separate Rec.2020-style terminal matrix and gamma drive an accurate on-device preview.
- **Live AR overlay of the corrected target.** Composites a perfectly-exposed, colour-correct rendition of the target back onto the physical target in frame, re-fished and oriented to match, with readout text rastered in.
- **Per-channel vignette correction (white & black).** Fits quadratic R/G/B black and white surfaces from anchor patches and removes lens brightness/black-level falloff before measuring colour.
- **Patch-quality / deviation flagging.** Flags dirty, noisy, or off-colour patches against brightness/chroma limits and intra-patch variance, warning when the target needs cleaning.
- **Re-solve from a saved scan under different settings.** Every scan's raw patch colours are saved and can be re-processed for a different illuminant/observer without re-shooting.
- **Illuminant selection by name, index, or Kelvin.** Choose the reference illuminant from a built-in catalogue by index or case-insensitive name prefix, or enter a raw Kelvin temperature directly — backed by a D65/D50 observer/whitepoint catalogue (e.g. "D65 – ISO/CIE 11664-2:2022").
- **Custom measured illuminant via Sekonic SPD import.** Loads a Sekonic C-7000 CSV (preferring the 1nm spectral block) as the reference illuminant SPD.
- **DaVinci Resolve DCTL export.** Writes a self-contained "VERICHROME DSR.dctl" you drag onto a Resolve clip to apply the solved scene-referred transform.

## Calibration (dark-frame)

- **On-device dark-frame & bias capture.** Capture a per-sensor calibration on the phone: a fast bias frame (shortest shutter) and a long dark frame (longest shutter, ~16s), both at max ISO, averaged over many exposures with poll-driven finalize in ~33ms.
- **Physics-based FPN + dark-current subtraction.** Removes each pixel's offset (scaled by sensor gain) and dark current (scaled by gain and exposure time), preserving the flat black pedestal so DNGs keep a normal BlackLevel tag.
- **Robust hot/unstable pixel detection (median + MAD).** Flags pixels whose dark level or frame-to-frame variance exceeds median + 6×(1.4826×MAD) over a strided ~300k-pixel sample — no hardcoded thresholds.
- **Bad-pixel reconstruction from same-CFA-phase neighbours.** Fills detected bad pixels from the mean of good same-colour neighbours at the 4×4 quad period, flagged in-place with a sentinel value (no separate mask allocation).
- **Self-describing VSF calibration file.** Each calibration is one labeled file carrying the mean and variance maps plus kind, dimensions, frame count, ISO, exposure, and black level; named per lens+resolution so different lenses don't collide.
- **On-device VSF read-back verification.** After write, the file is re-read and decoded on-device through a native verifier that sets explicit OK/FAIL signalling bits, so a corrupt or mis-saved calibration is caught immediately rather than at first use.
- **Firmware-proof capture metadata.** The exact ISO, exposure, and black level the cal was shot at are frozen into the file and never re-read from the sensor, so a HAL/firmware update can't corrupt the correction.
- **Built-in BLAKE3 integrity checksum.** A corrupt or tampered file is detected on load (with an inflate-length guard on the untrusted deflate blob) so it can't silently poison every corrected photo.
- **Per-format calibration application.** The correction is applied once to the raw before the format branch, so DNG, JPEG, TIFF, and JPEG XL all derive from one corrected buffer; cal files are re-decoded per save and freed (never held resident) to avoid OOM. Motion and slitscan are skipped.
- **Live split-half convergence metric.** A Pearson correlation of even- vs odd-frame averages over a deterministic ~300k-pixel sample, plus mean dark level and residual noise (~1/√N), published live so you know when it's converged.
- **Wall-clock arrival gating.** Rejects bogus early frames the HAL delivers before a forced exposure takes effect (dark: reject under half the forced shutter; bias: drop the first second of spin-up).
- **Calibration result freeze + tap-to-dismiss.** The averaged dark frame holds on screen and the feed freezes (so fast bias frames can't scribble over it) until you tap.
- **Calibration-failure reason surfaced to the UI.** A rejected scan (e.g. overexposure) records a human-readable reason the host UI shows on-device instead of failing silently.
- **Single master calibration reused across all modes.** One calibration is captured at the sensor's full resolution and derived (binned/cropped) for every mode of that lens.

## Demosaic & Image Quality

- **RCD demosaic for standard-Bayer exports.** A Rust port of Luis Sanz Rodriguez's RCD 2.3 — ratio-corrected, edge-directional, colour-difference-domain reconstruction for saved RGB.
- **Quad-Bayer (Tetracell) max-res demosaic.** A purpose-built RCD-derived algorithm for the 4×4 CFA, reconstructing green at every pixel directionally and R/B from the green-difference domain; validated against synthetic ground truth at ~33 dB edges / ~35 dB natural detail (rel-PSNR).
- **Diagonal (45-degree) edge interpolation.** The quad demosaic resolves 45-degree edges via synthesised diagonal green clusters, picking the lowest-gradient direction to interpolate along edges.
- **Colour-difference-domain reconstruction.** Both demosaics work in the green-/chroma-difference domain for clean, artefact-free saturated colours.
- **Two-tier demosaic quality (live vs save).** The live viewfinder uses a fast 2×2-block debayer; saved exports use the higher-quality RCD or quad demosaic, selected by the raw10/quad flag.
- **Quad-to-standard-Bayer binning for calibration.** Collapses each 2×2 same-colour cluster of a quad frame into one pixel so chameleon's 2×2-only debayer can read it for colour calibration.
- **16-bit / f32 internal precision.** Demosaicing runs in normalised f32; quad and binning paths deliberately preserve sub-black samples as real signal rather than clamping, to retain noise-floor information.
- **Border interpolation / edge handling.** Replicates interior pixels into the border so edges don't show coloured fringes.
- **Stochastic dithering on 8-bit output.** A deterministic per-pixel hash dither carries the fractional remainder up probabilistically, trading banding for perceptually-preferred noise (byte-identical on re-save).

## Output, Formats & Metadata

- **Four output formats.** Save each photo as 16-bit raw DNG, lossless 16-bit Deflate TIFF, JPEG (quality 95), or lossless JPEG XL, cycled from one on-screen control. (DNG is 16-bit raw and TIFF is 16-bit-per-channel RGB; JPEG/JXL are 8-bit RGB.)
- **16-bit lossless TIFF with error-diffusion dither.** TIFF exports the full f32 pipeline at 16-bit per channel, Deflate-compressed, with per-channel error-diffusion dithering on the f32→16-bit quantization so quantization residual becomes dither instead of banding.
- **16-bit DNG raw.** Uncompressed 16-bit-per-pixel raw with full white level 65535, mode-aware black level, and the raw buffer appended after the IFD.
- **Lossless JPEG XL with Rec.2020 + EXIF.** Encoded via a Lumis fork of zune-jpegxl that writes a real Rec.2020 colour encoding into the codestream, wrapped in an ISOBMFF box container to carry EXIF when present.
- **JPEG XL → JPEG fallback.** If a device's MediaStore rejects image/jxl, the photo saves as JPEG instead, with belt-and-suspenders guards at the cycle, at open, and at save.
- **DNG ColorMatrix1 (calibrated spectral colour).** Embeds the calibration-derived camera-to-XYZ matrix, the named scene-relative profile, a D50 illuminant, and a linear tone curve; uncalibrated devices fall back to an identity matrix so the DNG stays valid.
- **Quad-Bayer 4×4 CFA tagging.** Max-res raws are tagged with the real 4×4 quad-Bayer pattern (16-byte CFAPattern) so quad-aware converters can demosaic them; binned stays standard 2×2.
- **DNG embedded preview JPEG.** Each DNG embeds a self-built preview (binned, no demosaic) so any viewer or file browser thumbnails it instantly — fixing quad-Bayer DNGs that tools like RawTherapee, libraw, and Nemo can't read.
- **Rec.2020 wide-gamut tagging on all RGB exports.** JPEG (APP2 ICC), TIFF (ICCProfile tag), and JPEG XL (in-codestream) all carry a BT.2020-primaried, D50-adapted ICC/colour profile; JPEG marks ColorSpace=Uncalibrated since pixels are Rec.2020.
- **Effective/composite exposure metadata.** EXIF ExposureTime/ISO record the true integrated stack (effective ISO below 1 for large stacks), with a RATIONAL encoding that preserves sub-1s times.
- **Full EXIF block.** Exposure, ISO, f-number, focal length and 35mm-equivalent, subject distance, capture date — written in strict ascending tag order (exiftool-validate clean) into a proper EXIF sub-IFD.
- **GPS metadata (lat/lon/altitude).** Geotags photos with deg/min/sec RATIONAL triplets and altitude when a fix exists, written end-to-end into the DNG/TIFF GPS sub-IFD, omitted otherwise.
- **Orientation handling.** Device rotation (sensor mount + held angle) sets the EXIF/DNG Orientation tag, while RGB exports bake the rotation into the pixels so they're upright even in viewers that ignore the tag.
- **DNG BaselineExposure (non-destructive display gain).** The on-screen brightness you set is written as log2(gain) stops so DNGs open at that brightness without altering raw data; RGB exports bake it into pixels.
- **Human-readable exposure summary.** An ImageDescription / JPEG-comment string names the mode and both per-frame and effective exposure.
- **TIFF embedded thumbnail.** Appends a chained-IFD JPEG thumbnail so large TIFFs preview in Android galleries, falling back to a valid thumbnail-less TIFF on any failure.
- **Atomic file writes + safe filenames.** The standalone TIFF path writes to a temp file, fsyncs, and atomically renames; filenames use "YYYYMMDD HHMMSS mmm Mode" with no colons (illegal on Android/FUSE) and dedupe on the capture timestamp.

## Live Tools & Controls

- **Live 16-bit RAW preview in linear Rec.2020.** Reads the 16-bit-scaled RAW buffer, black-subtracts, applies display gain and a camera→Rec.2020 matrix in linear light, then sqrt-encodes to a BT.2020 surface.
- **Four live capture modes from one button.** Cycle Average (full-colour), Difference, Motion, and Slitscan in real time.
- **Live per-channel RAW histogram.** Per-Bayer-channel red/green/blue on a log2 brightness axis spanning the full dynamic range, with a flicker-free density curve and hard right-edge clip warnings.
- **Real-time per-channel clipping overlay.** Blown highlights crush to dark/false-colour and crushed shadows flash white (via a wrapping black subtract), per colour channel, when controls are visible.
- **Tap-to-zoom 1:1 raw-pixel inspector with pan.** Drag to a pixel-for-pixel view to inspect the CFA mosaic, hot pixels, and noise; tap to return.
- **Slitscan circular time-axis inspector.** In 1:1 the time axis wraps the ring buffer, so panning past the newest column rolls straight back to the oldest.
- **Manual ISO / shutter / focus sliders.** Log-scale ISO and shutter (reading 1/x or seconds) and a linear focus slider with metre/cm/infinity readout; focus greys out and goes inert on fixed-focus lenses.
- **Exposure-time slider with Minute→Year timebase.** Dial exposure from fractions of a second to whole years; switching timebase rescales the slider to preserve the actual duration.
- **Display-gain slider (0–12 stops).** Up to 12 stops of preview brightness, also carried into saved files.
- **Per-track increment arrows.** Each slider has left/right arrows for precise 1/256-step nudges.
- **Tap-toggle immersive view.** Tap the image to hide all controls for an unobstructed view, tap again to restore.
- **Continuous level / horizon indicator.** A three-dot triad shows roll and pitch straight from the gravity vector with zero smoothing, with signed degree readouts colour-coded by tilt.
- **Live corner readouts.** Stacked counters show frames saved, frame index, live FPS (coloured by ratio to the theoretical max), and the save format.
- **Elapsed/remaining exposure timers + progress bar.** Live elapsed and remaining timers with a coloured fill along the exposure slider that sweeps red→green by completion.
- **Save-format cycling by tapping the counter block.** Tap the corner counters to cycle JPEG XL / JPEG / DNG / TIFF, skipping JXL on unsupported devices.
- **On-device colour calibration tile.** Tap to scan a Verichrome target on a background thread (avoiding ANR); on success it rewrites the live preview matrix, DNG XYZ matrix, and ColorMatrix1, then persists settings.
- **Calibration result overlay + failure banner.** A colour-corrected reference patch composites onto the frozen frame on success; on failure an amber, word-wrapped reason banner appears and the feed auto-unfreezes — any touch clears it.
- **Calibrated 4-slot readout row.** Once calibrated, the top row shows the scanned target tile, target type/serial/life/UV/IR readout (with a gamma status or "CHECK TARGET" warning), the level, and the counters.
- **Dark/bias calibration screen + result view.** A dedicated screen shows live convergence, mean, noise, and a noise-vs-time graph with Finalize/Cancel; after finalize the captured noise is shown gamma-stretched with a saved-and-verified status.
- **Quad-Bayer-aware live rendering.** Resolves colour from 50MP quad-Bayer sensors by binning the 4×4 tile instead of rendering grey.
- **Orientation-aware UI.** The whole interface re-lays-out for portrait/landscape from the gravity vector, keeping text upright and rotating glyphs/widgets per orientation.
- **Partial-redraw engine with magic-pixel validation.** Repaints only changing readouts between frames using a sentinel-pixel buffer-validity check, falling back to a full draw when the buffer is stale.
- **Anti-aliased procedural widgets + Oxanium text.** Buttons, arrows, slider dots, and the calibration tile are procedurally shaded; text uses the embedded Oxanium font via cosmic-text with per-glyph alpha blending and rotation.

## Camera Picker / Menu

- **Per-lens camera picker.** The opening screen lists one rich, tappable spec card per physical lens.
- **Multi-mode drill-down per lens.** Tapping a lens opens a sub-screen of that lens's individual capture modes (binned, full-res, cropped), each issuing its own StartCamera.
- **Full-resolution and cropped-FOV mode selection.** Hidden max-res readouts and digitally cropped sub-FOV modes are exposed as selectable modes, colour-coded (green = max-res, red = cropped).
- **Rich per-mode spec card.** Megapixels, facing, diagonal FOV with a drawn wedge, shutter range, aperture, focal length, ISO range, focus range, a Bayer-pattern swatch, black/white levels, pixel pitch in microns, output dimensions, and the API/hardware-level tag.
- **To-scale sensor diagram.** Draws the physical sensor outline at true on-screen scale (from screen DPI), filled with a miniature Bayer tile and labelled with its diagonal in millimetres.
- **FOV wedge + Bayer swatch.** A radial wedge visualises each lens's angle of view, and a 2×2 swatch shows the actual CFA layout.
- **Hardware-level / API tagging.** Each camera is tagged LIMITED / RAW / LEGACY / RAW+ / EXTERNAL and tinted by hardware level.
- **Non-RAW camera handling.** Cameras that can't produce RAW are greyed out, labelled "No RAW," and refuse to start capture.
- **Per-lens dark-frame calibration launch.** Each lens's modes screen has a Calibrate row offering DARK or BIAS, plumbed through to open the camera in calibration mode.
- **Back navigation between screens.** A teal "< BACK" button steps back from the calibration picker to the modes and from modes to the main list; Exit (which terminates the process) is pinned to the bottom on every screen.
- **Press / release / drag-cancel touch handling.** Buttons highlight on press and cancel cleanly if you slide off before releasing.
- **Native-window rendering with partial redraw.** The menu draws directly into Android's surface buffer over a scaled splash background, validating buffer freshness with a sentinel pixel and repainting only the row that changed.

## System & Architecture

- **Dual-process camera/UI architecture.** The camera runs in its own `:camera` process while the UI runs in the main process, communicating over a Messenger/Binder channel, so a UI hiccup can never stall the sensor.
- **Shared-memory frame buffer (ashmem, zero-copy).** Full-resolution frames are passed between processes via a mapped ashmem region with no copying, with layout computed identically on both sides.
- **Heartbeat self-kill watchdog.** Every UI frame writes a timestamp to shared memory; if the UI stops drawing for ~1.6s the camera process calls libc::exit(0) — unless a continuous save is in progress.
- **Continuous render loop drives the heartbeat.** A vsync-paced Choreographer loop redraws every display frame, which doubles as the keep-alive signal.
- **Aggressive auto-nuke on focus loss.** Leaving the app fully exits within two seconds via moveTaskToBack + killProcess, leaving no recents entry.
- **Kill-switch on IPC failure.** A broken link to the camera process shuts the UI down rather than showing a broken screen.
- **Self-managed Telecom silencer.** Places a silent self-addressed VoIP "call" so notifications are muted (no sound, no vibration) while shooting without touching media volume, needing only the normal MANAGE_OWN_CALLS permission; Telecom auto-releases it the moment the app dies.
- **Permission gate launcher.** A tiny black launcher activity requests camera + location (plus storage on older Android) then hands off to the camera; the camera always opens even if location is denied (photos just save untagged).
- **Gravity-based screen rotation.** Device rotation is derived in Rust from the raw gravity sensor with a 7 m/s² threshold and hysteresis, not Android's rotation API.
- **Rec.2020 wide-gamut preview pipeline.** The activity sets wide-colour-gamut and minimal-post-processing, and Rust tags the native window buffers as BT.2020 full-range so SurfaceFlinger drives the panel directly.
- **Runtime feature detection for old Android.** Runs down to API 21 (Android 5) while resolving API 28+ display features via dlsym, degrading gracefully instead of crashing.
- **Foreground camera service.** Bound (not started) to avoid the Android 12+ background-start crash, declared foregroundServiceType=camera.
- **Immersive edge-to-edge fullscreen.** Hides status/nav bars and draws under camera cutouts, re-applying on every focus regain.
- **Hardware-button capture controls.** Physical volume keys save photos (down = start continuous save, up = single save / cancel), while Bluetooth remotes and keyboards act as a shutter that completes the exposure.
- **Touch latch for sub-frame taps.** Defers a quick DOWN..UP by one frame so Rust always sees the transition, keeping taps responsive even when each frame is slow at full resolution.
- **Device-bound calibration storage.** Calibration is stored in the app's private files and tied to the device via ANDROID_ID.
- **Active-camera reconnect on relaunch.** Reopening the app reconnects to an already-running camera, preserving an in-progress long exposure across a quick app switch.
- **Per-process panic capture.** Crashes are logged with the exact process (CAMERA or UI), file, and line.
- **Screen-on while shooting.** Keeps the screen on via FLAG_KEEP_SCREEN_ON during use.

---

## Coming / not yet shipped

These exist in the codebase but are partial, placeholder, or not yet wired into the live app — listed honestly so the shipped list stays accurate.

- **AMaZE demosaic (planned).** A full Rust port of the AMaZE algorithm exists but is orphaned — not in the module tree, with no callers — so it isn't part of any live or save path.
- **Region-scoped RCD preview (partial).** RCD can demosaic an arbitrary cropped region with correct Bayer phase, but the only current caller is the full-frame save path; the documented zoomed-in preview caller doesn't exist yet.
- **DNG XYZ export profile (partial).** The RGB-export XYZ matrix currently reuses the calibrated Rec.2020 terminal matrix as a stand-in (marked TODO) rather than a dedicated XYZ terminal profile. (The DNG ColorMatrix1 itself is properly chameleon-solved — only the RGB-export XYZ path is a stand-in.)
- **Overnight-averaged dark calibration (partial).** The architecture averages an unbounded number of frames with live convergence, but the project's design notes a single 16s dark is still noise-dominated and benefits from prolonged (overnight) averaging that isn't a one-shot.
- **Bias variance map (partial).** A per-pixel variance map is captured and stored for both files, but its only current consumer is dark-map bad-pixel detection; the bias variance isn't applied.
- **Wake-lock acquire / calibration-averaging maturity (partial).** A wake lock is constructed but never acquired — the screen-on guarantee currently comes from FLAG_KEEP_SCREEN_ON; the calibration intent is plumbed end-to-end while the averaging math continues to mature.
- **Offline button-texture generator (developer tool).** A standalone host tool (proto.rs) generates a target-style button texture; it's a dev/asset tool, not part of the on-device menu.
