// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
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

// ───────────────────────── pause state ─────────────────────────

#[derive(Default)]
struct PauseState {
    inner: Mutex<(Option<Instant>, Duration)>, // (paused since, total paused)
}

impl PauseState {
    fn is_paused(&self) -> bool {
        self.inner.lock().map(|g| g.0.is_some()).unwrap_or(false)
    }
    fn pause(&self) {
        if let Ok(mut g) = self.inner.lock() {
            if g.0.is_none() {
                g.0 = Some(Instant::now());
            }
        }
    }
    fn resume(&self) {
        if let Ok(mut g) = self.inner.lock() {
            if let Some(t) = g.0.take() {
                g.1 += t.elapsed();
            }
        }
    }
    /// Time since `start` with paused periods removed.
    fn effective(&self, start: Instant) -> Duration {
        let mut paused = Duration::ZERO;
        if let Ok(g) = self.inner.lock() {
            paused = g.1;
            if let Some(t) = g.0 {
                paused += t.elapsed();
            }
        }
        start.elapsed().saturating_sub(paused)
    }
}

// ───────────────────────── audio (mic + system sound) ─────────────────────────

type Queue = Arc<Mutex<VecDeque<f32>>>;

/// Converts any-rate / any-channel float audio into 48 kHz stereo (linear resampling).
struct Conv {
    ratio: f64,
    pos: f64,
    prev: [f32; 2],
    ch: usize,
}

impl Conv {
    fn push(&mut self, data: &[f32], out: &mut VecDeque<f32>) {
        let ch = self.ch.max(1);
        let frames = data.len() / ch;
        if frames == 0 {
            return;
        }
        let frame_at = |i: usize| -> [f32; 2] {
            let l = data[i * ch];
            let r = if ch > 1 { data[i * ch + 1] } else { l };
            [l, r]
        };
        // virtual sequence: s[0] = previous last frame, s[i + 1] = frame i
        let s = |i: usize, prev: [f32; 2]| if i == 0 { prev } else { frame_at(i - 1) };
        while self.pos + 1.0 <= frames as f64 {
            let i = self.pos.floor() as usize;
            let f = (self.pos - i as f64) as f32;
            let a = s(i, self.prev);
            let b = s(i + 1, self.prev);
            out.push_back(a[0] * (1.0 - f) + b[0] * f);
            out.push_back(a[1] * (1.0 - f) + b[1] * f);
            self.pos += self.ratio;
        }
        self.pos -= frames as f64;
        self.prev = frame_at(frames - 1);
    }
}

fn build_stream<T>(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    q: Queue,
) -> Result<cpal::Stream, Err>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut conv = Conv {
        ratio: cfg.sample_rate.0 as f64 / 48_000.0,
        pos: 0.0,
        prev: [0.0; 2],
        ch: cfg.channels as usize,
    };
    let mut tmp: Vec<f32> = Vec::new();
    let stream = dev.build_input_stream(
        cfg,
        move |data: &[T], _| {
            tmp.clear();
            tmp.extend(data.iter().map(|s| f32::from_sample(*s)));
            if let Ok(mut q) = q.lock() {
                conv.push(&tmp, &mut q);
            }
        },
        |e| eprintln!("audio stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

fn open_stream(dev: &cpal::Device, cfg: cpal::SupportedStreamConfig) -> Option<(cpal::Stream, Queue)> {
    let q: Queue = Arc::new(Mutex::new(VecDeque::new()));
    let conf = cfg.config();
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => build_stream::<f32>(dev, &conf, q.clone()),
        SampleFormat::I16 => build_stream::<i16>(dev, &conf, q.clone()),
        SampleFormat::U16 => build_stream::<u16>(dev, &conf, q.clone()),
        SampleFormat::I32 => build_stream::<i32>(dev, &conf, q.clone()),
        _ => return None,
    }
    .ok()?;
    stream.play().ok()?;
    Some((stream, q))
}

fn audio_thread(
    mic: Option<String>,
    sys: bool,
    pause: Arc<PauseState>,
    tx: mpsc::Sender<(i64, Vec<u8>)>,
    stop: Arc<AtomicBool>,
) {
    let host = cpal::default_host();
    let mut streams: Vec<cpal::Stream> = Vec::new();
    let mut queues: Vec<Queue> = Vec::new();

    if let Some(name) = mic {
        let dev = if name.is_empty() {
            host.default_input_device()
        } else {
            host.input_devices()
                .ok()
                .and_then(|mut d| d.find(|x| x.name().map(|n| n == name).unwrap_or(false)))
                .or_else(|| host.default_input_device())
        };
        if let Some(dev) = dev {
            if let Ok(cfg) = dev.default_input_config() {
                if let Some((s, q)) = open_stream(&dev, cfg) {
                    streams.push(s);
                    queues.push(q);
                }
            }
        }
    }
    if sys {
        // Input stream on the default OUTPUT device = WASAPI loopback (what you hear).
        if let Some(dev) = host.default_output_device() {
            if let Ok(cfg) = dev.default_output_config() {
                if let Some((s, q)) = open_stream(&dev, cfg) {
                    streams.push(s);
                    queues.push(q);
                }
            }
        }
    }

    let t0 = Instant::now();
    let mut sent: u64 = 0;
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(10));
        if pause.is_paused() {
            for qu in &queues {
                if let Ok(mut g) = qu.lock() {
                    g.clear();
                }
            }
            continue;
        }
        // wall-clock driven: loopback delivers nothing while silent, so never wait on it
        let target = (pause.effective(t0).as_secs_f64() * 48_000.0) as u64;
        if target <= sent {
            continue;
        }
        let n = (target - sent) as usize;
        let mut mix = vec![0f32; n * 2];
        for qu in &queues {
            if let Ok(mut g) = qu.lock() {
                let take = g.len().min(n * 2);
                for slot in mix.iter_mut().take(take) {
                    *slot += g.pop_front().unwrap_or(0.0);
                }
                let keep = 48_000 / 4 * 2;
                while g.len() > keep {
                    g.pop_front();
                }
            }
        }
        let mut bytes = Vec::with_capacity(n * 4);
        for v in mix {
            let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let ts = (sent as i64) * 10_000_000 / 48_000;
        sent += n as u64;
        if tx.send((ts, bytes)).is_err() {
            break;
        }
    }
    drop(streams);
}

struct AudioHandle {
    stop: Arc<AtomicBool>,
    rx: mpsc::Receiver<(i64, Vec<u8>)>,
    join: Option<std::thread::JoinHandle<()>>,
}

fn start_audio(mic: Option<String>, sys: bool, pause: Arc<PauseState>) -> Option<AudioHandle> {
    if mic.is_none() && !sys {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let join = std::thread::spawn(move || audio_thread(mic, sys, pause, tx, s2));
    Some(AudioHandle { stop, rx, join: Some(join) })
}

// ───────────────────────── text overlay (timestamp / caption) ─────────────────────────

fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '-' => [0, 0, 0, 0x1F, 0, 0, 0],
        ':' => [0, 0x04, 0x04, 0, 0x04, 0x04, 0],
        '.' => [0, 0, 0, 0, 0, 0x0C, 0x0C],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '_' => [0, 0, 0, 0, 0, 0, 0x1F],
        _ => [0; 7],
    }
}

fn draw_overlays(buf: &mut [u8], w: usize, h: usize, cfg: &Config) {
    let mut lines: Vec<String> = Vec::new();
    let cap = cfg.caption.trim().to_uppercase();
    if !cap.is_empty() {
        lines.push(cap);
    }
    if cfg.timestamp {
        lines.push(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
    }
    if lines.is_empty() || buf.len() < w * h * 4 {
        return;
    }
    let scale = ((h / 320).max(1) * cfg.overlay_scale.clamp(1, 4) as usize).min(12);
    let pad = 2 * scale;
    let line_h = 7 * scale + pad;
    let text_w = lines.iter().map(|l| l.chars().count() * 6 * scale).max().unwrap_or(0);
    let box_w = text_w + pad * 2;
    let box_h = lines.len() * line_h + pad;
    let margin = 6 * scale;
    let (x0, y0) = match cfg.overlay_pos.as_str() {
        "tl" => (margin, margin),
        "tr" => (w.saturating_sub(box_w + margin), margin),
        "bl" => (margin, h.saturating_sub(box_h + margin)),
        _ => (w.saturating_sub(box_w + margin), h.saturating_sub(box_h + margin)),
    };
    for y in y0..(y0 + box_h).min(h) {
        for x in x0..(x0 + box_w).min(w) {
            let i = ((h - 1 - y) * w + x) * 4;
            for c in 0..3 {
                buf[i + c] = (buf[i + c] as u32 * 2 / 5) as u8;
            }
            buf[i + 3] = 255;
        }
    }
    for (li, line) in lines.iter().enumerate() {
        let ty = y0 + pad + li * line_h;
        for (ci, ch) in line.chars().enumerate() {
            let g = glyph(ch);
            let tx = x0 + pad + ci * 6 * scale;
            for (r, row) in g.iter().enumerate() {
                for col in 0..5 {
                    if row & (0x10 >> col) == 0 {
                        continue;
                    }
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let (px, py) = (tx + col * scale + dx, ty + r * scale + dy);
                            if px < w && py < h {
                                let i = ((h - 1 - py) * w + px) * 4;
                                buf[i] = 255;
                                buf[i + 1] = 255;
                                buf[i + 2] = 255;
                                buf[i + 3] = 255;
                            }
                        }
                    }
                }
            }
        }
    }
}

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
    crop: Option<(u32, u32, u32, u32)>, // x, y, w, h inside the captured display
    mic: Option<String>,
    sys_audio: bool,
    timestamp: bool,
    caption: String,
    overlay_pos: String,
    overlay_scale: u32,
    pause: Arc<PauseState>,
}

impl Config {
    fn overlay_on(&self) -> bool {
        self.timestamp || !self.caption.trim().is_empty()
    }
    fn has_audio(&self) -> bool {
        self.mic.is_some() || self.sys_audio
    }
}

struct Recorder {
    encoder: Option<VideoEncoder>,
    cfg: Config,
    start: Option<Instant>,
    next_idx: u64,
    last: Vec<u8>, // last clean frame (no overlay)
    cur: Vec<u8>,
    work: Vec<u8>,
    have_last: bool,
    audio: Option<AudioHandle>,
}

fn ts(idx: u64, fps: u32) -> i64 {
    (idx as i64) * 10_000_000 / fps as i64
}

fn emit(enc: &mut VideoEncoder, clean: &[u8], work: &mut Vec<u8>, cfg: &Config, idx: u64) -> Result<(), Err> {
    if cfg.overlay_on() {
        work.clear();
        work.extend_from_slice(clean);
        draw_overlays(work, cfg.width as usize, cfg.height as usize, cfg);
        enc.send_frame_buffer(work, ts(idx, cfg.fps))?;
    } else {
        enc.send_frame_buffer(clean, ts(idx, cfg.fps))?;
    }
    Ok(())
}

/// Crop + scale a BGRA frame (with row padding) into a tightly packed BGRA buffer.
#[allow(clippy::too_many_arguments)]
fn scale_bgra(
    src: &[u8], pitch: usize, cx: usize, cy: usize, sw: usize, sh: usize,
    dst: &mut Vec<u8>, dw: usize, dh: usize,
) {
    dst.resize(dw * dh * 4, 0);
    if sw == 0 || sh == 0 {
        return;
    }
    if sw == dw && sh == dh {
        for y in 0..dh {
            let s = (cy + y) * pitch + cx * 4;
            let oy = dh - 1 - y;
            dst[oy * dw * 4..(oy + 1) * dw * 4].copy_from_slice(&src[s..s + dw * 4]);
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
        let b0 = (cy + y0) * pitch + cx * 4;
        let b1 = (cy + y1) * pitch + cx * 4;
        let r0 = &src[b0..b0 + sw * 4];
        let r1 = &src[b1..b1 + sw * 4];
        let oy = dh - 1 - dy;
        let out = &mut dst[oy * dw * 4..(oy + 1) * dw * 4];
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
        let sub = if c.hevc { VideoSettingsSubType::HEVC } else { VideoSettingsSubType::H264 };
        let audio_settings = if c.has_audio() {
            AudioSettingsBuilder::default().sample_rate(48_000).channel_count(2).bitrate(192_000)
        } else {
            AudioSettingsBuilder::default().disabled(true)
        };
        let encoder = VideoEncoder::new(
            VideoSettingsBuilder::new(c.width, c.height)
                .sub_type(sub)
                .frame_rate(c.fps)
                .bitrate(c.bitrate),
            audio_settings,
            ContainerSettingsBuilder::default(),
            &c.path,
        )?;
        let audio = start_audio(c.mic.clone(), c.sys_audio, c.pause.clone());
        Ok(Self {
            encoder: Some(encoder),
            cfg: c,
            start: None,
            next_idx: 0,
            last: Vec::new(),
            cur: Vec::new(),
            work: Vec::new(),
            have_last: false,
            audio,
        })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, _ctl: InternalCaptureControl) -> Result<(), Self::Error> {
        if let (Some(a), Some(enc)) = (self.audio.as_ref(), self.encoder.as_mut()) {
            while let Ok((t, b)) = a.rx.try_recv() {
                let _ = enc.send_audio_buffer(&b, t);
            }
        }
        let Some(enc) = self.encoder.as_mut() else { return Ok(()) };

        // "Fast" mode: GPU frame straight to the encoder (no region/audio/overlay).
        if !self.cfg.compat {
            enc.send_frame(frame)?;
            return Ok(());
        }
        if self.cfg.pause.is_paused() {
            return Ok(());
        }

        let start = *self.start.get_or_insert_with(Instant::now);
        let fps = self.cfg.fps;
        let idx = (self.cfg.pause.effective(start).as_nanos() * fps as u128 / 1_000_000_000) as u64;

        {
            let mut fb = frame.buffer()?;
            let (fw, fh, pitch) = (fb.width() as usize, fb.height() as usize, fb.row_pitch() as usize);
            let (cx, cy, cw, ch) = match self.cfg.crop {
                Some((x, y, w, h)) => {
                    let x = (x as usize).min(fw.saturating_sub(2));
                    let y = (y as usize).min(fh.saturating_sub(2));
                    (x, y, (w as usize).min(fw - x).max(2), (h as usize).min(fh - y).max(2))
                }
                None => (0, 0, fw, fh),
            };
            scale_bgra(
                fb.as_raw_buffer(), pitch, cx, cy, cw, ch,
                &mut self.cur, self.cfg.width as usize, self.cfg.height as usize,
            );
        }

        if self.have_last {
            while self.next_idx < idx {
                emit(enc, &self.last, &mut self.work, &self.cfg, self.next_idx)?;
                self.next_idx += 1;
            }
        }
        if idx >= self.next_idx {
            emit(enc, &self.cur, &mut self.work, &self.cfg, idx)?;
            self.next_idx = idx + 1;
        }
        std::mem::swap(&mut self.last, &mut self.cur);
        self.have_last = true;
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        if let Some(a) = self.audio.as_mut() {
            a.stop.store(true, Ordering::SeqCst);
            if let Some(j) = a.join.take() {
                let _ = j.join();
            }
        }
        if let (Some(a), Some(enc)) = (self.audio.as_ref(), self.encoder.as_mut()) {
            while let Ok((t, b)) = a.rx.try_recv() {
                let _ = enc.send_audio_buffer(&b, t);
            }
        }
        if self.cfg.compat && self.have_last {
            if let (Some(start), Some(enc)) = (self.start, self.encoder.as_mut()) {
                let fps = self.cfg.fps;
                let end = (self.cfg.pause.effective(start).as_nanos() * fps as u128 / 1_000_000_000) as u64;
                while self.next_idx <= end {
                    emit(enc, &self.last, &mut self.work, &self.cfg, self.next_idx)?;
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

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
struct Options {
    source: String, // "screen" | "region" | "window"
    monitor: usize,
    window: String,
    region_x: u32,
    region_y: u32,
    region_w: u32,
    region_h: u32,
    height: u32, // 0 = original size
    fps: u32,
    quality: String,
    codec: String,
    custom_mbps: Option<f64>,
    cursor: bool,
    border: bool,
    compat: bool,
    folder: String,
    audio_mic: bool,
    mic_device: String,
    audio_system: bool,
    timestamp: bool,
    caption: String,
    overlay_pos: String,
    overlay_scale: u32,
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
    pause: Arc<PauseState>,
}

#[derive(Default)]
struct AppState {
    active: Mutex<Option<Active>>,
}

// ───────────────────────── helpers ─────────────────────────

fn window_size(w: &Window) -> (u32, u32) {
    match w.rect() {
        Ok(r) => ((r.right - r.left).max(2) as u32, (r.bottom - r.top).max(2) as u32),
        Err(_) => (1920, 1080),
    }
}

fn find_window(title: &str) -> Option<Window> {
    Window::enumerate()
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.title().map(|t| t == title).unwrap_or(false))
}

fn monitor_size(index: usize) -> (u32, u32) {
    Monitor::enumerate()
        .ok()
        .and_then(|l| l.into_iter().nth(index))
        .map(|m| (m.width().unwrap_or(1920), m.height().unwrap_or(1080)))
        .unwrap_or((1920, 1080))
}

fn source_size(o: &Options) -> (u32, u32) {
    match o.source.as_str() {
        "window" => find_window(&o.window).map(|w| window_size(&w)).unwrap_or((1920, 1080)),
        "region" => (o.region_w.max(2), o.region_h.max(2)),
        _ => monitor_size(o.monitor),
    }
}

fn build_config(o: &Options, pause: Arc<PauseState>) -> Config {
    let (sw, sh) = source_size(o);
    let (w, h) = if o.height == 0 {
        ((sw & !1).max(2), (sh & !1).max(2))
    } else {
        (
            (((o.height as f64 * sw as f64 / sh as f64).round() as u32) & !1).max(2),
            (o.height & !1).max(2),
        )
    };
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
            ((w as f64 * h as f64 * fps as f64 * 0.09 * q * codec) as u64).clamp(150_000, 120_000_000) as u32
        }
    };
    let crop = if o.source == "region" {
        Some((o.region_x, o.region_y, o.region_w.max(2), o.region_h.max(2)))
    } else {
        None
    };
    let mic = if o.audio_mic { Some(o.mic_device.clone()) } else { None };
    let mut cfg = Config {
        path: format!(
            "{}\\SimpleCam_{}.mp4",
            o.folder.trim_end_matches('\\'),
            chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
        ),
        width: w,
        height: h,
        fps,
        bitrate,
        hevc: o.codec == "hevc",
        cursor: o.cursor,
        border: o.border,
        compat: o.compat,
        crop,
        mic,
        sys_audio: o.audio_system,
        timestamp: o.timestamp,
        caption: o.caption.clone(),
        overlay_pos: o.overlay_pos.clone(),
        overlay_scale: o.overlay_scale.max(1),
        pause,
    };
    // region, audio and overlays all need the Smooth (CPU) pipeline
    if cfg.crop.is_some() || cfg.has_audio() || cfg.overlay_on() {
        cfg.compat = true;
    }
    cfg
}

fn stop_active(app: &AppHandle) -> Option<String> {
    let state = app.state::<AppState>();
    let active = state.active.lock().ok()?.take();
    active.map(|a| {
        let _ = a.control.stop();
        a.path
    })
}

fn apply_hotkeys(app: &AppHandle, rec: &str, pause: &str) -> Result<(), String> {
    let gs = app.global_shortcut();
    gs.unregister_all().map_err(|e| e.to_string())?;
    gs.on_shortcut(rec, |app, _s, ev| {
        if ev.state() == ShortcutState::Pressed {
            let _ = app.emit("hotkey-record", ());
        }
    })
    .map_err(|e| e.to_string())?;
    if pause != rec {
        gs.on_shortcut(pause, |app, _s, ev| {
            if ev.state() == ShortcutState::Pressed {
                let _ = app.emit("hotkey-pause", ());
            }
        })
        .map_err(|e| e.to_string())?;
    }
    Ok(())
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
            MonitorInfo { index: i, name: format!("Display {} ({}x{})", i + 1, w, h), width: w, height: h }
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
fn list_audio_inputs() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|d| d.filter_map(|x| x.name().ok()).collect())
        .unwrap_or_default()
}

#[tauri::command]
fn default_folder() -> String {
    std::env::var("USERPROFILE")
        .map(|p| format!("{p}\\Videos\\SimpleCam"))
        .unwrap_or_else(|_| "SimpleCam".into())
}

#[tauri::command]
fn compute_output(opts: Options) -> OutputInfo {
    let c = build_config(&opts, Arc::new(PauseState::default()));
    OutputInfo { width: c.width, height: c.height, bitrate: c.bitrate }
}

#[tauri::command]
fn set_hotkeys(app: AppHandle, rec: String, pause: String) -> Result<(), String> {
    apply_hotkeys(&app, &rec, &pause)
}

#[tauri::command]
async fn start_recording(opts: Options, state: State<'_, AppState>) -> Result<String, String> {
    if state.active.lock().map_err(|e| e.to_string())?.is_some() {
        return Err("Already recording".into());
    }
    std::fs::create_dir_all(&opts.folder).map_err(|e| format!("Cannot create folder: {e}"))?;
    let pause = Arc::new(PauseState::default());
    let cfg = build_config(&opts, pause.clone());
    let cursor = if cfg.cursor { CursorCaptureSettings::WithCursor } else { CursorCaptureSettings::WithoutCursor };
    let border = if cfg.border { DrawBorderSettings::WithBorder } else { DrawBorderSettings::WithoutBorder };

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
    *state.active.lock().map_err(|e| e.to_string())? =
        Some(Active { control, path: cfg.path.clone(), started: Instant::now(), pause });
    Ok(cfg.path)
}

#[tauri::command]
async fn stop_recording(app: AppHandle) -> Result<String, String> {
    stop_active(&app).ok_or_else(|| "Not recording".to_string())
}

#[tauri::command]
fn pause_recording(state: State<'_, AppState>) -> Result<(), String> {
    let g = state.active.lock().map_err(|e| e.to_string())?;
    g.as_ref().map(|a| a.pause.pause()).ok_or_else(|| "Not recording".to_string())
}

#[tauri::command]
fn resume_recording(state: State<'_, AppState>) -> Result<(), String> {
    let g = state.active.lock().map_err(|e| e.to_string())?;
    g.as_ref().map(|a| a.pause.resume()).ok_or_else(|| "Not recording".to_string())
}

#[derive(Serialize)]
struct Status {
    recording: bool,
    paused: bool,
    elapsed: u64,
    size: u64,
}

#[tauri::command]
fn recording_status(state: State<'_, AppState>) -> Status {
    match state.active.lock().ok().as_deref() {
        Some(Some(a)) => Status {
            recording: true,
            paused: a.pause.is_paused(),
            elapsed: a.pause.effective(a.started).as_secs(),
            size: std::fs::metadata(&a.path).map(|m| m.len()).unwrap_or(0),
        },
        _ => Status { recording: false, paused: false, elapsed: 0, size: 0 },
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

#[tauri::command]
fn play_file(path: String) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", &path])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn();
}

/// Opens a transparent full-display overlay so the user can drag a capture region.
#[tauri::command]
async fn pick_region(app: AppHandle, monitor: usize) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("region") {
        let _ = w.close();
    }
    let mons = app.available_monitors().map_err(|e| e.to_string())?;
    let m = mons.get(monitor).or_else(|| mons.first()).ok_or("No display found")?;
    let (pos, size) = (*m.position(), *m.size());
    let win = tauri::WebviewWindowBuilder::new(&app, "region", tauri::WebviewUrl::App("region.html".into()))
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;
    let _ = win.set_position(tauri::Position::Physical(pos));
    let _ = win.set_size(tauri::Size::Physical(size));
    let _ = win.show();
    let _ = win.set_focus();
    Ok(())
}

#[tauri::command]
fn region_done(app: AppHandle, x: i32, y: i32, w: u32, h: u32, cancelled: bool) {
    if let Some(win) = app.get_webview_window("region") {
        let _ = win.close();
    }
    if !cancelled {
        let _ = app.emit("region-selected", serde_json::json!({ "x": x, "y": y, "w": w, "h": h }));
    }
    show_main(&app);
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
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            list_monitors,
            list_windows,
            list_audio_inputs,
            default_folder,
            compute_output,
            set_hotkeys,
            start_recording,
            stop_recording,
            pause_recording,
            resume_recording,
            recording_status,
            open_path,
            play_file,
            pick_region,
            region_done
        ])
        .setup(|app| {
            let _ = apply_hotkeys(app.handle(), "F9", "F10");

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
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { .. } = event {
                    stop_active(window.app_handle());
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running SimpleCam");
}
