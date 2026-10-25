// BrailCam 0.1.1 - lightweight screen recorder for Windows.
// Hide the console window in release builds.
#![windows_subsystem = "windows"]

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use global_hotkey::{
    hotkey::{Code, HotKey},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
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
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

type Err = Box<dyn std::error::Error + Send + Sync>;

// ------------------------------------------------------------------ audio --

/// A chunk of 16-bit little-endian PCM from the system "what you hear" mix.
struct AudioChunk {
    bytes: Vec<u8>,
    /// Time since recording started, in 100 ns units.
    ts: i64,
}

#[derive(Clone, Copy)]
struct AudioInfo {
    sample_rate: u32,
    channels: u32,
}

fn on_audio_err(e: cpal::StreamError) {
    eprintln!("audio stream error: {e}");
}

fn make_chunk(bytes: Vec<u8>, frames: usize, rate: u32, start: Instant) -> AudioChunk {
    let now = (start.elapsed().as_nanos() / 100) as i64;
    let len = (frames as i64 * 10_000_000) / rate.max(1) as i64;
    AudioChunk { bytes, ts: (now - len).max(0) }
}

/// Opens a WASAPI loopback stream on the default output device
/// (cpal treats an input stream on an output device as loopback).
fn open_loopback(
    tx: mpsc::Sender<AudioChunk>,
    start: Instant,
) -> Result<(cpal::Stream, AudioInfo), String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no output device found")?;
    let cfg = device.default_output_config().map_err(|e| e.to_string())?;
    let info = AudioInfo { sample_rate: cfg.sample_rate().0, channels: cfg.channels() as u32 };
    let format = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let (rate, ch) = (info.sample_rate, info.channels.max(1) as usize);

    let stream = match format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| {
                let bytes: Vec<u8> = data
                    .iter()
                    .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
                    .collect();
                let _ = tx.send(make_chunk(bytes, data.len() / ch, rate, start));
            },
            on_audio_err,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _| {
                let bytes: Vec<u8> = data.iter().flat_map(|s| s.to_le_bytes()).collect();
                let _ = tx.send(make_chunk(bytes, data.len() / ch, rate, start));
            },
            on_audio_err,
            None,
        ),
        other => return Err(format!("unsupported audio format {other:?}")),
    }
    .map_err(|e| e.to_string())?;

    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, info))
}

/// The audio stream is not `Send`, so it lives on its own thread until `stop` is set.
fn spawn_audio(
    stop: Arc<AtomicBool>,
    start: Instant,
    tx: mpsc::Sender<AudioChunk>,
) -> Result<(JoinHandle<()>, AudioInfo), String> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<AudioInfo, String>>();
    let handle = std::thread::spawn(move || match open_loopback(tx, start) {
        Ok((stream, info)) => {
            let _ = ready_tx.send(Ok(info));
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(40));
            }
            drop(stream);
        }
        Err(e) => {
            let _ = ready_tx.send(Err(e));
        }
    });
    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(info)) => Ok((handle, info)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("audio device did not respond".into()),
    }
}

// ---------------------------------------------------------------- capture --

struct Flags {
    path: String,
    stop: Arc<AtomicBool>,
    width: u32,
    height: u32,
    audio: Option<AudioInfo>,
    rx: Mutex<Option<mpsc::Receiver<AudioChunk>>>,
}

struct Capture {
    encoder: Option<VideoEncoder>,
    stop: Arc<AtomicBool>,
    rx: Option<mpsc::Receiver<AudioChunk>>,
}

impl Capture {
    fn drain_audio(&mut self) {
        if let (Some(enc), Some(rx)) = (self.encoder.as_mut(), self.rx.as_ref()) {
            while let Ok(chunk) = rx.try_recv() {
                let _ = enc.send_audio_buffer(&chunk.bytes, chunk.ts);
            }
        }
    }
}

impl GraphicsCaptureApiHandler for Capture {
    type Flags = Flags;
    type Error = Err;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let f = ctx.flags;
        let audio_settings = match f.audio {
            Some(a) => AudioSettingsBuilder::default()
                .sample_rate(a.sample_rate)
                .channel_count(a.channels),
            None => AudioSettingsBuilder::default().disabled(true),
        };
        let encoder = VideoEncoder::new(
            VideoSettingsBuilder::new(f.width, f.height).sub_type(VideoSettingsSubType::H264),
            audio_settings,
            ContainerSettingsBuilder::default(),
            &f.path,
        )?;
        let rx = f.rx.lock().unwrap().take();
        Ok(Self { encoder: Some(encoder), stop: f.stop, rx })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        self.drain_audio();
        if let Some(enc) = self.encoder.as_mut() {
            enc.send_frame(frame)?;
        }
        if self.stop.load(Ordering::SeqCst) {
            self.drain_audio();
            if let Some(enc) = self.encoder.take() {
                enc.finish()?;
            }
            control.stop();
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        // Finalise the MP4 if we were stopped before another frame arrived.
        self.drain_audio();
        if let Some(enc) = self.encoder.take() {
            enc.finish()?;
        }
        Ok(())
    }
}

// --------------------------------------------------------------- recorder --

struct Recorder {
    control: Option<CaptureControl<Capture, Err>>,
    audio_thread: Option<JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
    started: Option<Instant>,
    last_file: Option<PathBuf>,
    status: String,
    record_audio: bool,
}

fn output_dir() -> PathBuf {
    dirs::video_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("BrailCam")
}

impl Recorder {
    fn new() -> Self {
        Self {
            control: None,
            audio_thread: None,
            stop_flag: Arc::new(AtomicBool::new(false)),
            started: None,
            last_file: None,
            status: "Ready".into(),
            record_audio: true,
        }
    }

    fn is_recording(&self) -> bool {
        self.control.is_some()
    }

    fn start(&mut self) {
        if self.is_recording() {
            return;
        }
        match self.try_start() {
            Ok(note) => self.status = note,
            Err(e) => self.status = format!("Error: {e}"),
        }
    }

    fn try_start(&mut self) -> Result<String, Err> {
        let dir = output_dir();
        std::fs::create_dir_all(&dir)?;
        let name = format!("BrailCam_{}.mp4", chrono::Local::now().format("%Y%m%d_%H%M%S"));
        let path = dir.join(name);

        let monitor = Monitor::primary()?;
        // H.264 encoders want even dimensions.
        let width = monitor.width()? & !1;
        let height = monitor.height()? & !1;

        self.stop_flag = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let (tx, rx) = mpsc::channel::<AudioChunk>();

        // Try to start system-audio capture first; fall back to video only.
        let mut note = String::from("Recording...");
        let mut audio = None;
        if self.record_audio {
            match spawn_audio(self.stop_flag.clone(), started, tx) {
                Ok((handle, info)) => {
                    self.audio_thread = Some(handle);
                    audio = Some(info);
                    note = "Recording with system audio...".into();
                }
                Err(e) => note = format!("Recording (no audio: {e})"),
            }
        }

        let flags = Flags {
            path: path.to_string_lossy().into_owned(),
            stop: self.stop_flag.clone(),
            width,
            height,
            audio,
            rx: Mutex::new(Some(rx)),
        };
        let settings = Settings::new(
            monitor,
            CursorCaptureSettings::WithCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            flags,
        );

        match Capture::start_free_threaded(settings) {
            Ok(control) => {
                self.control = Some(control);
                self.started = Some(started);
                self.last_file = Some(path);
                Ok(note)
            }
            Err(e) => {
                // Make sure the audio thread doesn't linger.
                self.stop_flag.store(true, Ordering::SeqCst);
                if let Some(h) = self.audio_thread.take() {
                    let _ = h.join();
                }
                Err(e.into())
            }
        }
    }

    fn stop(&mut self) {
        if let Some(control) = self.control.take() {
            self.stop_flag.store(true, Ordering::SeqCst);
            self.status = match control.stop() {
                Ok(()) => "Saved".into(),
                Err(e) => format!("Stopped with error: {e}"),
            };
            if let Some(h) = self.audio_thread.take() {
                let _ = h.join();
            }
            self.started = None;
        }
    }

    fn toggle(&mut self) {
        if self.is_recording() {
            self.stop();
        } else {
            self.start();
        }
    }
}

fn open_folder() {
    let dir = output_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
}

// --------------------------------------------------------------------- ui --

struct App {
    rec: Arc<Mutex<Recorder>>,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(250));

        egui::CentralPanel::default().show(ctx, |ui| {
            let mut rec = self.rec.lock().unwrap();
            let recording = rec.is_recording();

            ui.vertical_centered(|ui| {
                ui.add_space(4.0);
                ui.heading(format!("BrailCam {VERSION}"));
                ui.add_space(4.0);

                let secs = rec.started.map(|s| s.elapsed().as_secs()).unwrap_or(0);
                let time = format!("{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60);
                let color = if recording { egui::Color32::LIGHT_RED } else { egui::Color32::GRAY };
                ui.label(egui::RichText::new(time).monospace().size(26.0).color(color));
                ui.label(rec.status.as_str());
                if !recording {
                    if let Some(name) = rec.last_file.as_ref().and_then(|p| p.file_name()) {
                        ui.small(name.to_string_lossy());
                    }
                }
                ui.add_space(6.0);

                ui.add_enabled(!recording, egui::Checkbox::new(&mut rec.record_audio, "Record system audio"));
                ui.add_space(4.0);

                let label = if recording { "Stop  (F9)" } else { "Record  (F9)" };
                if ui.add_sized([180.0, 34.0], egui::Button::new(label)).clicked() {
                    rec.toggle();
                }
                ui.add_space(4.0);
                if ui.add_sized([180.0, 26.0], egui::Button::new("Open folder  (F10)")).clicked() {
                    open_folder();
                }
            });
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.rec.lock().unwrap().stop();
    }
}

// ------------------------------------------------------------------- main --

fn main() -> eframe::Result<()> {
    let rec = Arc::new(Mutex::new(Recorder::new()));

    // Global hotkeys. The manager must live on the UI thread and stay alive.
    let manager = GlobalHotKeyManager::new().expect("hotkey manager");
    let f9 = HotKey::new(None, Code::F9);
    let f10 = HotKey::new(None, Code::F10);
    let _ = manager.register(f9);
    let _ = manager.register(f10);

    let rec_for_keys = rec.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        if e.state != HotKeyState::Pressed {
            return;
        }
        if e.id == f9.id() {
            rec_for_keys.lock().unwrap().toggle();
        } else if e.id == f10.id() {
            open_folder();
        }
    }));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([280.0, 250.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_always_on_top(),
        ..Default::default()
    };

    let result = eframe::run_native(
        &format!("BrailCam {VERSION}"),
        options,
        Box::new(move |_cc| Ok(Box::new(App { rec }))),
    );
    drop(manager);
    result
}
