# BrailCam 0.1.1

Lightweight Windows screen recorder in Rust. Records the primary monitor to MP4 (H.264) using the
Windows Graphics Capture API, plus system audio ("what you hear") via WASAPI loopback.

## Hotkeys (work even when the window is unfocused)
- **F9**  start / stop recording
- **F10** open the output folder

Recordings are saved to `Videos\BrailCam\BrailCam_YYYYMMDD_HHMMSS.mp4`.

## Changelog
- **0.1.1** - system audio capture (toggle in the window), H.264 output, version shown in the title
- **0.1.0** - first release: screen capture, hotkeys, simple UI

## Build
Push to GitHub and the included Actions workflow builds `brailcam.exe` (artifact `BrailCam-windows`).
Locally: install Rust (MSVC) and run `cargo build --release`.

Requires Windows 10 1903+ / Windows 11.
