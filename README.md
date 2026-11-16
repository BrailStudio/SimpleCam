<div align="center">

<img src="ui/logo.png" width="140" alt="SimpleCam logo" />

# SimpleCam

**Ultra lightweight screen recorder for Windows.**
Record a full display or a single window, from 144p up to 4K, to MP4.

Built with Rust + [Tauri 2](https://tauri.app) and the Windows Graphics Capture API.

</div>

---

## Features

- **Full screen or single window** capture with a one-click switch
- **Pick any attached display** (multi-monitor supported) or any open window
- **144p to 4K** output: 144p, 240p, 360p, 480p, 720p, 1080p, 1440p, 2160p (aspect ratio is preserved)
- **15 / 24 / 30 / 60 / 120 fps**
- **H.264** (most compatible) or **HEVC / H.265** (smaller files)
- **Quality presets** (Low / Medium / High) or a **custom bitrate** (0.5 to 100 Mbps)
- **Global hotkey: F9** to start and stop, even when SimpleCam is in the background
- **Countdown** before recording (off / 3 / 5 / 10 seconds)
- Cursor on/off, yellow capture border on/off, minimize while recording, keep on top
- System tray icon (Show / Quit)
- Settings are remembered between launches
- Hardware-accelerated encoding through Media Foundation, so CPU use stays low
- **Install and uninstall wizards** (NSIS setup `.exe` and `.msi`)

## Requirements

- Windows 10 version 1903 or newer, or Windows 11
- WebView2 runtime (preinstalled on Windows 11; the installer downloads it on Windows 10 if missing)

## Install

1. Go to the **Releases** page of this repository and download:
   - `SimpleCam_x.y.z_x64-setup.exe` (recommended, wizard installer), or
   - `SimpleCam_x.y.z_x64_en-US.msi` (for managed / enterprise installs)
2. Run the installer. The wizard walks you through:
   - language selection
   - license agreement
   - install scope: **just for me** (no admin needed) or **all users**
   - install folder and Start Menu folder
   - optional desktop shortcut and launch-on-finish
3. Windows SmartScreen may warn that the app is from an unknown publisher, because the installer is not code-signed. Choose **More info, then Run anyway**.

## Uninstall

Any of these works:

- **Settings > Apps > Installed apps > SimpleCam > Uninstall**
- **Start Menu > SimpleCam > Uninstall**
- Re-run the setup `.exe` and choose uninstall

The uninstall wizard offers a checkbox to **also delete app data** (saved settings). Your recorded videos are never touched.

## Usage

1. Choose **Full screen** or **Single window**.
2. Pick the display or window, then resolution, frame rate, codec and quality.
3. Press **Start recording** (or **F9**).
4. Press **Stop recording** (or **F9** again). The MP4 is saved to the folder shown in **Save to** (default `Videos\SimpleCam`).

Notes:

- The window dropdown has a refresh button; use it after opening new apps.
- If the source resolution is smaller than the chosen output, the video is upscaled.
- Window capture records that window only, even if other windows cover it.
- Bitrate presets use about 0.09 bits per pixel per frame at Medium (Low is 0.5x, High is 1.8x, HEVC is 0.65x).

## Build from source

Prerequisites: [Rust](https://rustup.rs) (stable, MSVC toolchain), [Node.js](https://nodejs.org) 18+, and the Visual Studio C++ Build Tools.

```powershell
npm install
npm run dev        # run in development
npm run build      # build NSIS + MSI installers
```

Installers appear in `src-tauri/target/release/bundle/`:

| File | Location |
| --- | --- |
| Setup wizard (`.exe`) | `bundle/nsis/` |
| MSI installer | `bundle/msi/` |
| Portable exe | `src-tauri/target/release/simplecam.exe` |

Other scripts: `npm run build:nsis`, `npm run build:msi`, and `npm run icons` (regenerates all icon sizes from `app-icon.png`).

## Build on GitHub (CI)

The workflow in `.github/workflows/build.yml` builds on a Windows runner.

1. Push this project to a GitHub repository.
2. Every push to `main`, every pull request, and manual runs (**Actions > Build SimpleCam > Run workflow**) produce a downloadable artifact named `SimpleCam-windows`.
3. To publish a release, tag a version:

   ```powershell
   git tag v0.3.0
   git push origin v0.3.0
   ```

   The workflow builds and attaches the installers to a new GitHub Release automatically.

Keep the version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` in sync when releasing.

## Project structure

```
SimpleCam/
├─ package.json              npm scripts + Tauri CLI
├─ app-icon.png              source icon (npm run icons)
├─ LICENSE
├─ ui/                       frontend (plain HTML / CSS / JS, no bundler)
│  ├─ index.html
│  ├─ style.css
│  ├─ main.js
│  └─ logo.png
├─ src-tauri/
│  ├─ Cargo.toml
│  ├─ tauri.conf.json        app + installer (NSIS / MSI) configuration
│  ├─ capabilities/default.json
│  ├─ icons/
│  └─ src/main.rs            capture, encoder, commands, tray, hotkey
└─ .github/workflows/build.yml
```

## Troubleshooting

- **Yellow border around the screen:** enable/disable it with "Show yellow capture border" (Windows 11 can hide it; Windows 10 may always show it).
- **Black video when capturing a window:** some protected or hardware-overlay apps (DRM video, some games in exclusive fullscreen) cannot be captured. Use borderless windowed mode.
- **F9 does nothing:** another app may already own that key.
- **No audio:** audio capture is not implemented yet (see roadmap).

## Roadmap

- System audio and microphone capture (WASAPI)
- Pause / resume
- Region capture
- Configurable hotkey

## License

MIT, see [LICENSE](LICENSE).
