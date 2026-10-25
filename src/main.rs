// Hide the console window in release builds.
#![windows_subsystem = "windows"]

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use eframe::egui;
use global_hotkey::{
    hotkey::{Code, HotKey},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    encoder::{AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{ColorFormat, CursorCaptureSettings, DrawBorderSettings, Settings},
};

type Err = Box<dyn std::error::Error + Send + Sync>;

// ---------------------------------------------------------------- capture --

struct Flags {
    path: String,
    stop: Arc<AtomicBool>,
    width: u32,
    height: u32,
}

struct Capture {
    encoder: Option<VideoEncoder>,
    stop: Arc<AtomicBool>,
}

impl GraphicsCaptureApiHandler for Capture {
    type Flags = Flags;
    type Error = Err;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let f = ctx.flags;
        let encoder = VideoEncoder::new(
            VideoSettingsBuilder::new(f.width, f.height),
            AudioSettingsBuilder::default().disabled(true),
            ContainerSettingsBuilder::default(),
            &f.path,
        )?;
        Ok(Self { encoder: Some(encoder), stop: f.stop })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if let Some(enc) = self.encoder.as_mut() {
            enc.send_frame(frame)?;
        }
        if self.stop.load(Ordering::SeqCst) {
            if let Some(enc) = self.encoder.take() {
                enc.finish()?;
            }
            control.stop();
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        // Finalise the MP4 if we were stopped before another frame arrived.
        if let Some(enc) = self.encoder.take() {
            enc.finish()?;
        }
        Ok(())
    }
}

// --------------------------------------------------------------- recorder --

struct Recorder {
    control: Option<CaptureControl<Capture, Err>>,
    stop_flag: Arc<AtomicBool>,
    started: Option<Instant>,
    last_file: Option<PathBuf>,
    status: String,
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
            stop_flag: Arc::new(AtomicBool::new(false)),
            started: None,
            last_file: None,
            status: "Ready".into(),
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
            Ok(()) => self.status = "Recording...".into(),
            Err(e) => self.status = format!("Error: {e}"),
        }
    }

    fn try_start(&mut self) -> Result<(), Err> {
        let dir = output_dir();
        std::fs::create_dir_all(&dir)?;
        let name = format!("BrailCam_{}.mp4", chrono::Local::now().format("%Y%m%d_%H%M%S"));
        let path = dir.join(name);

        let monitor = Monitor::primary()?;
        // H.264 encoders want even dimensions.
        let width = monitor.width()? & !1;
        let height = monitor.height()? & !1;

        self.stop_flag = Arc::new(AtomicBool::new(false));
        let flags = Flags {
            path: path.to_string_lossy().into_owned(),
            stop: self.stop_flag.clone(),
            width,
            height,
        };
        let settings = Settings::new(
            monitor,
            CursorCaptureSettings::WithCursor,
            DrawBorderSettings::WithoutBorder,
            ColorFormat::Bgra8,
            flags,
        );

        let control = Capture::start_free_threaded(settings)?;
        self.control = Some(control);
        self.started = Some(Instant::now());
        self.last_file = Some(path);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(control) = self.control.take() {
            self.stop_flag.store(true, Ordering::SeqCst);
            self.status = match control.stop() {
                Ok(()) => "Saved".into(),
                Err(e) => format!("Stopped with error: {e}"),
            };
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
                ui.add_space(6.0);
                ui.heading("BrailCam");
                ui.add_space(6.0);

                let secs = rec.started.map(|s| s.elapsed().as_secs()).unwrap_or(0);
                let time = format!("{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60);
                let color = if recording { egui::Color32::LIGHT_RED } else { egui::Color32::GRAY };
                ui.label(egui::RichText::new(time).monospace().size(26.0).color(color));
                ui.label(rec.status.as_str());
                ui.add_space(8.0);

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
            .with_inner_size([260.0, 190.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_always_on_top(),
        ..Default::default()
    };

    let result = eframe::run_native(
        "BrailCam",
        options,
        Box::new(move |_cc| Ok(Box::new(App { rec }))),
    );
    drop(manager);
    result
}
