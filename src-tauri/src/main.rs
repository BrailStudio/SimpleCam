// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::{
    sync::Mutex,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    encoder::{
        AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder,
        VideoSettingsSubType,
    },
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

type Err = Box<dyn std::error::Error + Send + Sync>;

// ───────────────────────── capture handler ─────────────────────────

#[derive(Clone)]
struct Config {
    path: String,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
    hevc: bool,
    cursor: bool,
    border: bool,
    compat: bool,
}

struct Recorder {
    encoder: Option<VideoEncoder>,
    cfg: Config,
    start: Option<Instant>,
    next_idx: u64,
    last: Vec<u8>,
    cur: Vec<u8>,
    have_last: bool,
}

/// Presentation timestamp (100 ns units) of frame number `idx` at a constant frame rate.
fn ts(idx: u64, fps: u32) -> i64 {
    (idx as i64) * 10_000_000 / fps as i64
}

/// Copy / scale a BGRA frame (with row padding) into a tightly packed BGRA buffer.
fn scale_bgra(src: &[u8], sw: usize, sh: usize, pitch: usize, dst: &mut Vec<u8>, dw: usize, dh: usize) {
    dst.resize(dw * dh * 4, 0);
    if sw == 0 || sh == 0 {
        return;
    }
    if sw == dw && sh == dh {
        for y in 0..dh {
            let s = y * pitch;
            dst[y * dw * 4..(y + 1) * dw * 4].copy_from_slice(&src[s..s + dw * 4]);
        }
        return;
    }
    let xr = ((sw as u64) << 16) / dw as u64;
    let yr = ((sh as u64) << 16) / dh as u64;
    let mut xs: Vec<(usize, usize, u32)> = Vec::with_capacity(dw);
    for dx in 0..dw {
        let fx = (dx as u64 * xr + xr / 2).saturating_sub(1 << 15);
        let x0 = ((fx >> 16) as usize).min(sw - 1);
        let x1 = (x0 + 1).min(sw - 1);
        xs.push((x0 * 4, x1 * 4, ((fx >> 8) & 0xff) as u32));
    }
    for dy in 0..dh {
        let fy = (dy as u64 * yr + yr / 2).saturating_sub(1 << 15);
        let y0 = ((fy >> 16) as usize).min(sh - 1);
        let y1 = (y0 + 1).min(sh - 1);
        let wy = ((fy >> 8) & 0xff) as u32;
        let r0 = &src[y0 * pitch..y0 * pitch + sw * 4];
        let r1 = &src[y1 * pitch..y1 * pitch + sw * 4];
        let out = &mut dst[dy * dw * 4..(dy + 1) * dw * 4];
        for (dx, &(a, b, wx)) in xs.iter().enumerate() {
            let o = dx * 4;
            for c in 0..4 {
                let top = r0[a + c] as u32 * (256 - wx) + r0[b + c] as u32 * wx;
                let bot = r1[a + c] as u32 * (256 - wx) + r1[b + c] as u32 * wx;
                out[o + c] = ((top * (256 - wy) + bot * wy) >> 16) as u8;
            }
        }
    }
}

impl GraphicsCaptureApiHandler for Recorder {
    type Flags = Config;
    type Error = Err;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let c = ctx.flags;
        let sub = if c.hevc {
            VideoSettingsSubType::HEVC
        } else {
            VideoSettingsSubType::H264
        };
        let encoder = VideoEncoder::new(
            VideoSettingsBuilder::new(c.width, c.height)
                .sub_type(sub)
                .frame_rate(c.fps)
                .bitrate(c.bitrate),
            AudioSettingsBuilder::default().disabled(true),
            ContainerSettingsBuilder::default(),
            &c.path,
        )?;
        Ok(Self {
            encoder: Some(encoder),
            cfg: c,
            start: None,
            next_idx: 0,
            last: Vec::new(),
            cur: Vec::new(),
            have_last: false,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _ctl: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let Some(enc) = self.encoder.as_mut() else {
            return Ok(());
        };

        // "Fast" mode: hand the GPU frame straight to the encoder.
        if !self.cfg.compat {
            enc.send_frame(frame)?;
            return Ok(());
        }

        // "Smooth" mode: system-memory BGRA frames + constant frame rate.
        // Windows only delivers a frame when the screen changes, so gaps are
        // filled by repeating the previous frame at exact intervals.
        let start = *self.start.get_or_insert_with(Instant::now);
        let fps = self.cfg.fps;
        let idx = (start.elapsed().as_nanos() * fps as u128 / 1_000_000_000) as u64;

        {
            let mut fb = frame.buffer()?;
            let (sw, sh, pitch) = (fb.width() as usize, fb.height() as usize, fb.row_pitch() as usize);
            scale_bgra(
                fb.as_raw_buffer(),
                sw,
                sh,
                pitch,
                &mut self.cur,
                self.cfg.width as usize,
                self.cfg.height as usize,
            );
        }

        if self.have_last {
            while self.next_idx < idx {
                enc.send_frame_buffer(&self.last, ts(self.next_idx, fps))?;
                self.next_idx += 1;
            }
        }
        if idx >= self.next_idx {
            enc.send_frame_buffer(&self.cur, ts(idx, fps))?;
            self.next_idx = idx + 1;
        }
        std::mem::swap(&mut self.last, &mut self.cur);
        self.have_last = true;
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        if self.cfg.compat && self.have_last {
            if let (Some(start), Some(enc)) = (self.start, self.encoder.as_mut()) {
                let fps = self.cfg.fps;
                let end = (start.elapsed().as_nanos() * fps as u128 / 1_000_000_000) as u64;
                while self.next_idx <= end {
                    enc.send_frame_buffer(&self.last, ts(self.next_idx, fps))?;
                    self.next_idx += 1;
                }
            }
        }
        if let Some(e) = self.encoder.take() {
            e.finish()?;
        }
        Ok(())
    }
}

// ───────────────────────── shared types ─────────────────────────

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Options {
    source: String, // "screen" | "window"
    monitor: usize,
    window: String,
    height: u32,
    fps: u32,
    quality: String, // "low" | "medium" | "high"
    codec: String,   // "h264" | "hevc"
    custom_mbps: Option<f64>,
    cursor: bool,
    border: bool,
    compat: bool,
    folder: String,
}

#[derive(Serialize)]
struct MonitorInfo {
    index: usize,
    name: String,
    width: u32,
    height: u32,
}

#[derive(Serialize)]
struct WindowInfo {
    title: String,
    width: u32,
    height: u32,
}

#[derive(Serialize)]
struct OutputInfo {
    width: u32,
    height: u32,
    bitrate: u32,
}

struct Active {
    control: CaptureControl<Recorder, Err>,
    path: String,
    started: Instant,
}

#[derive(Default)]
struct AppState {
    active: Mutex<Option<Active>>,
}

// ───────────────────────── helpers ─────────────────────────

fn window_size(w: &Window) -> (u32, u32) {
    match w.rect() {
        Ok(r) => (
            (r.right - r.left).max(2) as u32,
            (r.bottom - r.top).max(2) as u32,
        ),
        Err(_) => (1920, 1080),
    }
}

fn find_window(title: &str) -> Option<Window> {
    Window::enumerate()
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.title().map(|t| t == title).unwrap_or(false))
}

fn source_size(o: &Options) -> (u32, u32) {
    if o.source == "window" {
        if let Some(w) = find_window(&o.window) {
            return window_size(&w);
        }
        return (1920, 1080);
    }
    Monitor::enumerate()
        .ok()
        .and_then(|l| l.into_iter().nth(o.monitor))
        .map(|m| (m.width().unwrap_or(1920), m.height().unwrap_or(1080)))
        .unwrap_or((1920, 1080))
}

fn build_config(o: &Options) -> Config {
    let (sw, sh) = source_size(o);
    let h = (o.height & !1).max(2);
    let w = (((o.height as f64 * sw as f64 / sh as f64).round() as u32) & !1).max(2);
    let fps = o.fps.clamp(1, 240);
    let bitrate = match o.custom_mbps {
        Some(m) => (m.clamp(0.1, 200.0) * 1_000_000.0) as u32,
        None => {
            let q = match o.quality.as_str() {
                "low" => 0.5,
                "high" => 1.8,
                _ => 1.0,
            };
            let codec = if o.codec == "hevc" { 0.65 } else { 1.0 };
            let bps = w as f64 * h as f64 * fps as f64 * 0.09 * q * codec;
            (bps as u64).clamp(150_000, 120_000_000) as u32
        }
    };
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Config {
        path: format!("{}\\SimpleCam_{}.mp4", o.folder.trim_end_matches('\\'), secs),
        width: w,
        height: h,
        fps,
        bitrate,
        hevc: o.codec == "hevc",
        cursor: o.cursor,
        border: o.border,
        compat: o.compat,
    }
}

fn stop_active(app: &AppHandle) -> Option<String> {
    let state = app.state::<AppState>();
    let active = state.active.lock().ok()?.take();
    active.map(|a| {
        let _ = a.control.stop();
        a.path
    })
}

// ───────────────────────── commands ─────────────────────────

#[tauri::command]
fn list_monitors() -> Vec<MonitorInfo> {
    Monitor::enumerate()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let (w, h) = (m.width().unwrap_or(0), m.height().unwrap_or(0));
            MonitorInfo {
                index: i,
                name: format!("Display {} ({}x{})", i + 1, w, h),
                width: w,
                height: h,
            }
        })
        .collect()
}

#[tauri::command]
fn list_windows() -> Vec<WindowInfo> {
    let mut v: Vec<WindowInfo> = Window::enumerate()
        .unwrap_or_default()
        .iter()
        .filter(|w| w.is_valid())
        .filter_map(|w| {
            let title = w.title().ok()?;
            if title.trim().is_empty() {
                return None;
            }
            let (width, height) = window_size(w);
            Some(WindowInfo { title, width, height })
        })
        .collect();
    v.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    v.dedup_by(|a, b| a.title == b.title);
    v
}

#[tauri::command]
fn default_folder() -> String {
    std::env::var("USERPROFILE")
        .map(|p| format!("{p}\\Videos\\SimpleCam"))
        .unwrap_or_else(|_| "SimpleCam".into())
}

#[tauri::command]
fn compute_output(opts: Options) -> OutputInfo {
    let c = build_config(&opts);
    OutputInfo { width: c.width, height: c.height, bitrate: c.bitrate }
}

#[tauri::command]
async fn start_recording(opts: Options, state: State<'_, AppState>) -> Result<String, String> {
    if state.active.lock().map_err(|e| e.to_string())?.is_some() {
        return Err("Already recording".into());
    }
    std::fs::create_dir_all(&opts.folder).map_err(|e| format!("Cannot create folder: {e}"))?;
    let cfg = build_config(&opts);
    let cursor = if cfg.cursor {
        CursorCaptureSettings::WithCursor
    } else {
        CursorCaptureSettings::WithoutCursor
    };
    let border = if cfg.border {
        DrawBorderSettings::WithBorder
    } else {
        DrawBorderSettings::WithoutBorder
    };

    macro_rules! launch {
        ($item:expr) => {
            Recorder::start_free_threaded(Settings::new(
                $item,
                cursor,
                border,
                SecondaryWindowSettings::Default,
                MinimumUpdateIntervalSettings::Default,
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                cfg.clone(),
            ))
        };
    }

    let result = if opts.source == "window" {
        let w = find_window(&opts.window).ok_or("Window not found. Refresh the list.")?;
        launch!(w)
    } else {
        let m = Monitor::enumerate()
            .ok()
            .and_then(|l| l.into_iter().nth(opts.monitor))
            .or_else(|| Monitor::primary().ok())
            .ok_or("No display found")?;
        launch!(m)
    };

    let control = result.map_err(|e| format!("Failed to start: {e}"))?;
    *state.active.lock().map_err(|e| e.to_string())? = Some(Active {
        control,
        path: cfg.path.clone(),
        started: Instant::now(),
    });
    Ok(cfg.path)
}

#[tauri::command]
async fn stop_recording(app: AppHandle) -> Result<String, String> {
    stop_active(&app).ok_or_else(|| "Not recording".to_string())
}

#[derive(Serialize)]
struct Status {
    recording: bool,
    elapsed: u64,
    path: Option<String>,
}

#[tauri::command]
fn recording_status(state: State<'_, AppState>) -> Status {
    match state.active.lock().ok().as_deref() {
        Some(Some(a)) => Status {
            recording: true,
            elapsed: a.started.elapsed().as_secs(),
            path: Some(a.path.clone()),
        },
        _ => Status { recording: false, elapsed: 0, path: None },
    }
}

#[tauri::command]
fn open_path(path: String) {
    let p = std::path::Path::new(&path);
    let mut cmd = std::process::Command::new("explorer");
    if p.is_file() {
        cmd.arg(format!("/select,{path}"));
    } else {
        cmd.arg(path);
    }
    let _ = cmd.spawn();
}

// ───────────────────────── app ─────────────────────────

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let _ = app.emit("hotkey", ());
                    }
                })
                .build(),
        )
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            list_monitors,
            list_windows,
            default_folder,
            compute_output,
            start_recording,
            stop_recording,
            recording_status,
            open_path
        ])
        .setup(|app| {
            // F9 = start / stop (ignored silently if another app owns the key)
            let _ = app.global_shortcut().register("F9");

            // system tray
            let show = MenuItem::with_id(app, "show", "Show SimpleCam", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .tooltip("SimpleCam")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main(app),
                    "quit" => {
                        stop_active(app);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Make sure the MP4 is finalized when the window is closed.
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                stop_active(window.app_handle());
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running SimpleCam");
}
