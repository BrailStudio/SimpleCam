<div align="center">

<img src="ui/logo.png" width="130" alt="SimpleCam logo" />

# SimpleCam

**Simple, lightweight, portable screen recorder for Windows.**
One `.exe`. No installer. Record a monitor, a window or a region, to MP4, from 144p up to 4K.

Rust + [Tauri 2](https://tauri.app) + Windows Graphics Capture

</div>

---

## Download and run

1. Open the **Releases** page (or the latest **Actions** run, artifact `SimpleCam-portable`).
2. Download **`SimpleCam.exe`** and double-click it. Nothing is installed.

To remove it, delete `SimpleCam.exe` and the `SimpleCam_data` folder next to it. Recordings are saved to `Videos\SimpleCam` and are never touched.

Windows may show a SmartScreen warning because the exe is not code-signed: **More info, then Run anyway**.

## Features

| | |
| --- | --- |
| **Flexible capture** | Whole monitor (pick any display), a single window, or a drag-selected region |
| **Intuitive controls** | Global shortcuts: start, stop and pause without touching the window |
| **High-quality output** | H.264 / AVC or H.265 / HEVC in MP4, constant frame rate, correct colors |
| **Resolution control** | 144p, 240p, 360p, 480p, 720p, 1080p, 1440p, 2160p (4K) or original size; 15 to 120 fps; presets |
| **Crystal-clear audio** | AAC 48 kHz stereo; **microphone** and **system sound** mixed, each with its own volume |
| **Customizable** | Hide the mouse cursor, show / hide the recording border, countdown, minimize while recording |
| **Limit recording** | Auto-stop after N seconds and / or N megabytes |
| **Lightweight** | Native Rust, single small exe, minimal memory |

Extras: pause / resume, date-time stamp and caption overlay, bitrate control (0 = automatic), always-on-top, play-when-finished, recording timer in the title bar, settings saved next to the exe.

## Shortcuts (editable)

| Action | Default |
| --- | --- |
| Capture Monitor | `Ctrl+PrintScreen` |
| Capture Window (the app you are using) | `Ctrl+Alt+PrintScreen` |
| Capture Region (drag, then recording starts) | `Ctrl+Shift+PrintScreen` |
| Pause / Resume | `Ctrl+Shift+P` |

Press a capture shortcut again to stop. Type any combination such as `Ctrl+Shift+F9` in the Shortcuts box.

## Requirements

- Windows 10 (1903 or newer) or Windows 11, 64-bit
- WebView2 runtime (built into Windows 11; on Windows 10 install it from Microsoft if the window stays blank)

## Build

You need [Rust](https://rustup.rs) (MSVC toolchain), [Node.js](https://nodejs.org) 18+ and the Visual Studio C++ Build Tools.

```powershell
npm install
npm run dev        # development
npm run build      # creates src-tauri\target\release\simplecam.exe
```

`npm run icons` regenerates the icon set from `app-icon.png`.

### Build on GitHub

The workflow in `.github/workflows/build.yml` builds on every push and uploads `SimpleCam.exe` as an artifact. To publish a release:

```powershell
git tag v1.0.0
git push origin v1.0.0
```

## Project layout

```
package.json  app-icon.png  LICENSE  README.md
ui/            index.html  style.css  main.js  region.html  logo.png
src-tauri/     Cargo.toml  tauri.conf.json  capabilities/  icons/  src/main.rs
.github/workflows/build.yml
```

## Not included (yet)

- Recording audio from one specific application only
- AV1 video, FLAC audio, fragmented MP4
- Cursor highlight, webcam overlay, annotations

## Troubleshooting

- **Black video from a window:** protected content (DRM video) and some exclusive-fullscreen games cannot be captured. Use borderless windowed mode.
- **A shortcut does nothing:** another program owns that key combination. Pick another one.
- **High CPU at 4K / 60 fps:** lower the frame rate or resolution.

## License

MIT, see [LICENSE](LICENSE).
