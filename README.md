# BrailCam

Lightweight Windows screen recorder in Rust. Records the primary monitor to MP4 (H.264) using the
Windows Graphics Capture API.

## Hotkeys (work even when the window is unfocused)
- **F9**  start / stop recording
- **F10** open the output folder

Recordings are saved to `Videos\BrailCam\BrailCam_YYYYMMDD_HHMMSS.mp4`.

## Build
1. Install Rust (MSVC toolchain) from https://rustup.rs and the VS Build Tools ("Desktop development with C++").
2. In this folder run: `cargo build --release`
3. Run `target\release\brailcam.exe`

Requires Windows 10 1903+ / Windows 11.
