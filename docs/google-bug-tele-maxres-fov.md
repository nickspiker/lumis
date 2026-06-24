# Android Camera2 bug report — Pixel 8 Pro telephoto MAXIMUM_RESOLUTION FOV mismatch

## For the issuetracker.google.com form

**Component:** Android Public Tracker > App Development > Camera (Camera2 / CameraX). On issuetracker, search the component picker for "Camera" — the relevant one is the platform Camera2 component (often listed as "Camera" under the Android Public Tracker). If a Pixel-specific component is offered ("Pixel > Camera"), use that, since this is a Pixel HAL behaviour.

**Title:** `Pixel 8 Pro: telephoto physical camera (id 6) MAXIMUM_RESOLUTION RAW stream delivers wrong FOV vs its reported CameraCharacteristics`

## Summary

On the Pixel 8 Pro, the rear telephoto physical camera (`physicalId = 6`, focal length 18.0 mm) advertises a `MAXIMUM_RESOLUTION` RAW10 stream (8064×6048) whose **delivered field of view does not match the static `CameraCharacteristics` for that physical camera**. The characteristics describe an ~11° diagonal FOV, but the frames actually delivered in maximum-resolution mode are ~22° — i.e. the HAL silently serves the max-res stream via an internal "FOV transition" to a wider sensor, while the reported `SENSOR_INFO_PHYSICAL_SIZE` and `LENS_INFO_AVAILABLE_FOCAL_LENGTHS` continue to describe the narrow native sensor.

This violates the Camera2 contract that a physical camera's FOV (derived from `SENSOR_INFO_PHYSICAL_SIZE` + `LENS_FOCAL_LENGTH`) applies to all of that camera's output streams, including the `MAXIMUM_RESOLUTION` configuration, which is documented as the same sensor read without binning (same FOV, ~4× pixels).

## Device

- Pixel 8 Pro
- Android 14+ (reproduced on the build present 2026-06; Camera HAL "Lyric")
- Logical rear camera `id = 0` (multi-fov), physical members include `RearTeleVirtual (id 6)`

## Observed characteristics (enumerated via CameraCharacteristics)

| physicalId | focal (mm) | SENSOR_INFO_PHYSICAL_SIZE (mm) | PIXEL_ARRAY | PIXEL_ARRAY_MAXIMUM_RESOLUTION | computed diagonal FOV |
|-----------:|-----------:|-------------------------------:|------------:|-------------------------------:|----------------------:|
| 2 (wide)   | 6.9        | 9.792 × 7.3728                 | 4080×3072   | 8160×6144                      | 83.2° |
| 3 (uwide)  | 2.23       | 6.4 × 4.8                      | 4000×3000   | 8000×6000                      | 121.7° |
| 4 (tele)   | 18.0       | 5.6448 × 4.2336                | 4032×3024   | (none)                         | 22.2° |
| **6 (tele-virtual)** | **18.0** | **2.8224 × 2.1168**   | **4032×3024** | **8064×6048**               | **11.2°** |

FOV = `2·atan(diagonal / (2·focal))` from the reported physical size and focal length.

## The bug

For `physicalId = 6`:

- **Binned mode** (`SENSOR_PIXEL_MODE_DEFAULT`, 4032×3024): delivers the advertised ~11° FOV. Correct.
- **Maximum-resolution mode** (`SENSOR_PIXEL_MODE_MAXIMUM_RESOLUTION`, RAW10 8064×6048): delivers ~22° FOV — visibly a wider framing than binned mode of the *same* physical camera, which is physically impossible for a genuine non-binned readout of the same sensor.

The HAL log confirms an internal FOV transition rather than a true native max-res readout of camera 6:

```
multicam_controller_base.cc: FOV transition participant camera: RearTeleVirtual (id: 6)
StreamInfo ... camera_id: RearTeleVirtual, format: RAW10, width: 8064, height: 6048, intended_for_max_resolution_mode: 1
zuma_fc_bayer_context: camera 6 ... sensor output size: 8064 x 6048 bayer output size: 4032 x 3024
```

So `RearTeleVirtual` cannot read its own sensor at 8064×6048; the HAL fulfils the max-res request by transitioning to a wider sensor and presenting its crop, but **does not update the physical camera's reported FOV characteristics to reflect the delivered stream.**

## Expected behaviour

One of:

1. `physicalId = 6` should **not advertise** a `MAXIMUM_RESOLUTION` RAW configuration it cannot deliver at its own native FOV; or
2. If it does advertise one, the delivered frames must match the camera's reported `SENSOR_INFO_PHYSICAL_SIZE` / focal-length FOV (true non-binned readout of that sensor); or
3. The `MAXIMUM_RESOLUTION`-variant characteristics (`SENSOR_INFO_PHYSICAL_SIZE_MAXIMUM_RESOLUTION` if applicable, active/pixel array max-res) must describe the *delivered* FOV so an app can compute it correctly.

Currently an app computing FOV from the documented characteristics gets 11° for a stream that is actually ~22°, with no characteristic indicating the discrepancy.

## Impact

A manual RAW camera app that labels lenses by computed FOV, or that maps screen coordinates to sensor coordinates by FOV, mislabels and mis-frames the telephoto max-res stream. There is no programmatic signal in `CameraCharacteristics` that the max-res stream of physical camera 6 is FOV-transitioned, so the app cannot compensate without device-specific hardcoding.

## Related Pixel Camera2 quirks observed on the same device

- Max-resolution RAW on the rear sensors is exposed **only as RAW10** (`ImageFormat.RAW10`, format 37), never as `RAW_SENSOR` (32).
- `REQUEST_AVAILABLE_CAPABILITIES` does **not** advertise `ULTRA_HIGH_RESOLUTION_SENSOR` despite the sensors exposing `*_MAXIMUM_RESOLUTION` configs.
