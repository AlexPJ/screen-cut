//! Grabación en Windows: Windows.Graphics.Capture entrega los fotogramas (de un
//! monitor o de una ventana, aunque la tapen) y el codificador de Media
//! Foundation de `windows-capture` escribe el MP4 (H.264 + AAC) por hardware.
//! El audio del sistema (loopback) y el micrófono se mezclan en una sola pista.
//! Los controles flotantes de ScreenCut no salen en el vídeo porque se crean
//! con `content_protected` (WDA_EXCLUDEFROMCAPTURE).

use super::{video_bitrate, Options, Source};
use crate::infra::audio::device::{self, AudioStream, Source as AudioSource};
use crate::infra::audio::mix::{self, Mixer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};
use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::encoder::{
    AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder, VideoSettingsSubType,
};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, GraphicsCaptureItemType,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const FPS: u32 = 30;
/// Intervalo mínimo entre fotogramas, en unidades de 100 ns.
const FRAME_INTERVAL: i64 = 10_000_000 / FPS as i64;
/// El audio se entrega con este retraso, margen para que lleguen los bloques
/// de las fuentes antes de mezclarlos.
const AUDIO_LATENCY: Duration = Duration::from_millis(40);

struct Shared {
    encoder: Mutex<Option<VideoEncoder>>,
    /// Momento del primer fotograma: el cero del vídeo y del audio.
    t0: Mutex<Option<Instant>>,
    /// Región que se recorta de cada fotograma (x0, y0, x1, y1), en píxeles.
    crop: Option<(u32, u32, u32, u32)>,
    on_error: Box<dyn Fn(String) + Send + Sync>,
}

struct Handler {
    shared: Arc<Shared>,
    last: Option<i64>,
    flipped: Vec<u8>,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = Arc<Shared>;
    type Error = BoxError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { shared: ctx.flags, last: None, flipped: Vec::new() })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, _control: InternalCaptureControl) -> Result<(), Self::Error> {
        // Windows entrega hasta la frecuencia del monitor; con 30 fps basta.
        let timestamp = frame.timestamp()?.Duration;
        if self.last.is_some_and(|last| timestamp - last < FRAME_INTERVAL - FRAME_INTERVAL / 10) {
            return Ok(());
        }
        let mut guard = self.shared.encoder.lock().unwrap_or_else(|e| e.into_inner());
        let Some(encoder) = guard.as_mut() else { return Ok(()) };
        match self.shared.crop {
            None => encoder.send_frame(frame)?,
            Some((x0, y0, x1, y1)) => {
                if x1 > frame.width() || y1 > frame.height() {
                    return Ok(()); // el monitor cambió de resolución
                }
                // El recorte pasa por la CPU; este camino del codificador
                // espera BGRA con las filas de abajo arriba.
                let buffer = frame.buffer_crop(x0, y0, x1, y1)?;
                let mut packed = Vec::new();
                let pixels = buffer.as_nopadding_buffer(&mut packed);
                let row = ((x1 - x0) * 4) as usize;
                self.flipped.clear();
                for line in pixels.chunks_exact(row).rev() {
                    self.flipped.extend_from_slice(line);
                }
                encoder.send_frame_buffer(&self.flipped, timestamp)?;
            }
        }
        drop(guard);
        self.last = Some(timestamp);
        let mut t0 = self.shared.t0.lock().unwrap_or_else(|e| e.into_inner());
        if t0.is_none() {
            *t0 = Some(Instant::now());
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        (self.shared.on_error)("La ventana que se grababa se ha cerrado".into());
        Ok(())
    }
}

/// Grabación en curso.
pub struct Recording {
    control: Option<CaptureControl<Handler, BoxError>>,
    shared: Arc<Shared>,
    streams: Vec<AudioStream>,
    pump: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
}

/// Empieza a capturar `item`. Quitar el borde amarillo solo se puede en
/// Windows 11; en Windows 10 se graba con él.
fn launch<T>(item: T, shared: &Arc<Shared>) -> Result<CaptureControl<Handler, BoxError>, String>
where
    T: TryInto<GraphicsCaptureItemType> + Copy + Send + 'static,
{
    let settings = |border| {
        Settings::new(
            item,
            CursorCaptureSettings::WithCursor,
            border,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            shared.clone(),
        )
    };
    Handler::start_free_threaded(settings(DrawBorderSettings::WithoutBorder))
        .or_else(|_| Handler::start_free_threaded(settings(DrawBorderSettings::Default)))
        .map_err(|e| format!("No se pudo empezar a grabar: {e:?}"))
}

fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

impl Recording {
    pub fn start(opts: Options, on_error: impl Fn(String) + Send + Sync + 'static) -> Result<(Self, Vec<String>), String> {
        let mut warnings = Vec::new();
        enum Item {
            Monitor(Monitor),
            Window(Window),
        }
        let (item, (width, height), crop) = match &opts.source {
            Source::Display { screen, rect } => {
                let center = POINT { x: screen.x + screen.width / 2, y: screen.y + screen.height / 2 };
                // SAFETY: MonitorFromPoint no tiene requisitos; con NEAREST siempre devuelve uno.
                let handle = unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST) };
                let monitor = Monitor::from_raw_hmonitor(handle.0 as *mut std::ffi::c_void);
                match rect {
                    None => {
                        let err = |e| format!("No se pudo leer el monitor: {e:?}");
                        let size = (monitor.width().map_err(err)?, monitor.height().map_err(err)?);
                        (Item::Monitor(monitor), (even(size.0), even(size.1)), None)
                    }
                    Some(r) => {
                        let (x, y) = (r.x.max(0.0) as u32, r.y.max(0.0) as u32);
                        let (w, h) = (even(r.width as u32), even(r.height as u32));
                        (Item::Monitor(monitor), (w, h), Some((x, y, x + w, y + h)))
                    }
                }
            }
            Source::Window { id } => {
                let window = Window::from_raw_hwnd(*id as usize as *mut std::ffi::c_void);
                if !window.is_valid() {
                    return Err("La ventana ya no está abierta".into());
                }
                let err = |e| format!("No se pudo leer la ventana: {e:?}");
                let size = (window.width().map_err(err)?.max(2) as u32, window.height().map_err(err)?.max(2) as u32);
                (Item::Window(window), (even(size.0), even(size.1)), None)
            }
        };

        let with_audio = opts.system_audio || opts.microphone;
        let _ = std::fs::remove_file(&opts.path);
        let encoder = VideoEncoder::new(
            VideoSettingsBuilder::new(width, height)
                .sub_type(VideoSettingsSubType::H264)
                .bitrate(video_bitrate(width as usize, height as usize) as u32)
                .frame_rate(FPS),
            AudioSettingsBuilder::new()
                .bitrate(128_000)
                .channel_count(2)
                .sample_rate(mix::RATE)
                .bit_per_sample(16)
                .disabled(!with_audio),
            ContainerSettingsBuilder::new(),
            &opts.path,
        )
        .map_err(|e| format!("No se pudo crear el vídeo: {e}"))?;
        let shared = Arc::new(Shared {
            encoder: Mutex::new(Some(encoder)),
            t0: Mutex::new(None),
            crop,
            on_error: Box::new(on_error),
        });

        // Audio: cada fuente se mezcla en una sola pista estéreo de 48 kHz.
        let mixer = Arc::new(Mutex::new(Mixer::new(2)));
        let mut streams = Vec::new();
        let sources = [(opts.system_audio, AudioSource::System), (opts.microphone, AudioSource::Microphone)];
        for (i, (wanted, source)) in sources.into_iter().enumerate() {
            if !wanted {
                continue;
            }
            let mixer = mixer.clone();
            match device::start(source, move |block| mixer.lock().unwrap_or_else(|e| e.into_inner()).push(i, &block)) {
                Ok(stream) => streams.push(stream),
                Err(e) => warnings.push(e),
            }
        }
        let pump = with_audio.then(|| {
            let stop = Arc::new(AtomicBool::new(false));
            let thread = std::thread::spawn({
                let (stop, shared) = (stop.clone(), shared.clone());
                move || pump_audio(&shared, &mixer, &stop)
            });
            (stop, thread)
        });

        let control = match item {
            Item::Monitor(m) => launch(m, &shared),
            Item::Window(w) => launch(w, &shared),
        };
        let mut recording = Self { control: None, shared, streams, pump };
        match control {
            Ok(control) => recording.control = Some(control),
            Err(e) => {
                let _ = recording.stop();
                let _ = std::fs::remove_file(&opts.path);
                return Err(e);
            }
        }
        Ok((recording, warnings))
    }

    /// Detiene la captura y cierra el MP4.
    pub fn stop(mut self) -> Result<(), String> {
        if let Some(control) = self.control.take() {
            let _ = control.stop();
        }
        for stream in self.streams.drain(..) {
            stream.stop();
        }
        if let Some((stop, thread)) = self.pump.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
        }
        let started = self.shared.t0.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        let encoder = self.shared.encoder.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(encoder) = encoder else { return Ok(()) };
        encoder.finish().map_err(|e| format!("No se pudo terminar de escribir el vídeo: {e}"))?;
        if !started {
            return Err("No se grabó ningún fotograma".into());
        }
        Ok(())
    }
}

/// Entrega el audio mezclado al codificador al ritmo del reloj, empezando con
/// el primer fotograma. El codificador fecha el audio contando muestras, así
/// que hay que mandarlo sin huecos: lo que falte va como silencio.
fn pump_audio(shared: &Shared, mixer: &Mutex<Mixer>, stop: &AtomicBool) {
    let mut sent: u64 = 0;
    let mut started = false;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(10));
        let Some(t0) = *shared.t0.lock().unwrap_or_else(|e| e.into_inner()) else { continue };
        if !started {
            // Lo que llegó antes del primer fotograma no entra en el vídeo.
            mixer.lock().unwrap_or_else(|e| e.into_inner()).clear();
            started = true;
        }
        let due = (t0.elapsed().saturating_sub(AUDIO_LATENCY).as_secs_f64() * mix::RATE as f64) as u64;
        if due <= sent {
            continue;
        }
        let frames = (due - sent) as usize;
        let pcm = mixer.lock().unwrap_or_else(|e| e.into_inner()).pull(frames);
        sent = due;
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut encoder = shared.encoder.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(encoder) = encoder.as_mut() {
            if encoder.send_audio_buffer(&bytes, 0).is_err() {
                return;
            }
        }
    }
}
