//! Grabación en macOS: ScreenCaptureKit entrega los fotogramas y el audio (del
//! sistema y, desde macOS 15, del micrófono) y AVAssetWriter los codifica a
//! MP4 por hardware. ScreenCut se excluye del filtro, así que sus ventanas
//! (el control de grabación incluido) nunca salen en el vídeo.

use super::{output_size, video_bitrate, Options, Source};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, AllocAnyThread, DefinedClass};
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterStatus, AVFileTypeMPEG4, AVMediaTypeAudio, AVMediaTypeVideo,
    AVVideoAverageBitRateKey, AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoCompressionPropertiesKey,
    AVVideoExpectedSourceFrameRateKey, AVVideoHeightKey, AVVideoMaxKeyFrameIntervalKey, AVVideoProfileLevelH264HighAutoLevel,
    AVVideoProfileLevelKey, AVVideoWidthKey,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_media::{CMClock, CMSampleBuffer, CMTime};
use objc2_foundation::{NSCopying, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSString, NSURL};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamOutput,
    SCStreamOutputType,
};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FPS: i32 = 30;
/// `kCVPixelFormatType_32BGRA`.
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
/// `kAudioFormatMPEG4AAC`.
const AAC: u32 = u32::from_be_bytes(*b"aac ");

/// Estado del escritor, compartido con los callbacks de ScreenCaptureKit (que
/// llegan todos por la misma cola serie).
struct Writer {
    writer: Retained<AVAssetWriter>,
    video: Retained<AVAssetWriterInput>,
    system: Option<Retained<AVAssetWriterInput>>,
    mic: Option<Retained<AVAssetWriterInput>>,
    /// La sesión del escritor empieza con el primer fotograma.
    started: bool,
    stopped: bool,
}

struct Shared {
    writer: Mutex<Writer>,
    /// Aviso cuando el sistema corta la captura (p. ej. se cierra la ventana).
    on_error: Box<dyn Fn(String) + Send + Sync>,
}

// SAFETY: los objetos de AVFoundation solo se tocan con el mutex tomado.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Shared {
    fn on_sample(&self, buffer: &CMSampleBuffer, kind: SCStreamOutputType) {
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: el buffer es válido durante el callback.
        unsafe {
            if w.stopped || !buffer.is_valid() {
                return;
            }
            if kind == SCStreamOutputType::Screen {
                // Los fotogramas "idle" (la pantalla no cambió) no traen imagen.
                if buffer.image_buffer().is_none() {
                    return;
                }
                if !w.started {
                    if !w.writer.startWriting() {
                        let error = w.writer.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default();
                        w.stopped = true;
                        drop(w);
                        (self.on_error)(format!("No se pudo empezar a escribir el vídeo: {error}"));
                        return;
                    }
                    w.writer.startSessionAtSourceTime(buffer.presentation_time_stamp());
                    w.started = true;
                }
                if w.video.isReadyForMoreMediaData() {
                    w.video.appendSampleBuffer(buffer);
                }
                return;
            }
            // El audio anterior al primer fotograma no cabe en el vídeo.
            let input = match kind {
                SCStreamOutputType::Audio => w.system.as_ref(),
                _ => w.mic.as_ref(),
            };
            if let (true, Some(input)) = (w.started, input) {
                if input.isReadyForMoreMediaData() {
                    input.appendSampleBuffer(buffer);
                }
            }
        }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ScreenCutStreamOutput"]
    #[ivars = Arc<Shared>]
    struct StreamOutput;

    unsafe impl NSObjectProtocol for StreamOutput {}

    unsafe impl SCStreamOutput for StreamOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn did_output(&self, _stream: &SCStream, buffer: &CMSampleBuffer, kind: SCStreamOutputType) {
            self.ivars().on_sample(buffer, kind);
        }
    }

    unsafe impl SCStreamDelegate for StreamOutput {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            let message = error.localizedDescription().to_string();
            (self.ivars().on_error)(format!("La grabación se detuvo: {message}"));
        }
    }
);

impl StreamOutput {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(shared);
        // SAFETY: init de NSObject.
        unsafe { msg_send![super(this), init] }
    }
}

/// Grabación en curso.
pub struct Recording {
    stream: Retained<SCStream>,
    output: Retained<StreamOutput>,
    _queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: SCStream es seguro entre hilos; el resto se usa a través de `Shared`.
unsafe impl Send for Recording {}

/// Espera a un completion handler de ScreenCaptureKit que solo informa de un error.
fn wait_for(f: impl FnOnce(&RcBlock<dyn Fn(*mut NSError)>)) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: si no es nulo, es un NSError válido durante el callback.
        let message = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
        let _ = tx.send(message);
    });
    f(&block);
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(None) => Ok(()),
        Ok(Some(message)) => Err(message),
        Err(_) => Err("El sistema no respondió".into()),
    }
}

fn shareable_content() -> Result<Retained<SCShareableContent>, String> {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        // SAFETY: punteros válidos (o nulos) durante el callback; se retiene el contenido.
        let result = match unsafe { Retained::retain(content) } {
            Some(content) => Ok(content),
            None => Err(unsafe { error.as_ref() }
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or_else(|| "sin detalles".into())),
        };
        let _ = tx.send(SendBox(result));
    });
    // SAFETY: el bloque vive hasta que se llama.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(SendBox(Ok(content))) => Ok(content),
        Ok(SendBox(Err(e))) => Err(format!(
            "No se pudo acceder a la pantalla ({e}). Revisa el permiso de Grabación de pantalla en Ajustes del Sistema."
        )),
        Err(_) => Err("El sistema no respondió al pedir las pantallas".into()),
    }
}

/// Para pasar objetos de Objective-C por un canal (se retienen y se sueltan
/// en otro hilo, que es seguro).
struct SendBox<T>(T);
unsafe impl<T> Send for SendBox<T> {}

/// Para los valores de un NSDictionary heterogéneo.
fn any<T: objc2::Message>(obj: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: todo objeto de Objective-C es un AnyObject.
    unsafe { Retained::cast_unchecked(obj) }
}

fn number(v: f64) -> Retained<AnyObject> {
    any(NSNumber::new_f64(v))
}

fn string(s: &NSString) -> Retained<AnyObject> {
    any(s.copy())
}

fn dict(pairs: &[(&NSString, Retained<AnyObject>)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<&NSString> = pairs.iter().map(|(k, _)| *k).collect();
    let values: Vec<Retained<AnyObject>> = pairs.iter().map(|(_, v)| v.clone()).collect();
    NSDictionary::from_retained_objects(&keys, &values)
}

fn key(k: Option<&'static NSString>) -> Result<&'static NSString, String> {
    k.ok_or_else(|| "AVFoundation no está disponible".to_string())
}

fn audio_input(channels: u32) -> Result<Retained<AVAssetWriterInput>, String> {
    // Las claves de AVAudioSettings.h valen lo mismo que su nombre.
    let settings = dict(&[
        (&NSString::from_str("AVFormatIDKey"), number(AAC as f64)),
        (&NSString::from_str("AVSampleRateKey"), number(48_000.0)),
        (&NSString::from_str("AVNumberOfChannelsKey"), number(channels as f64)),
        (&NSString::from_str("AVEncoderBitRateKey"), number(128_000.0)),
    ]);
    // SAFETY: tipo de medio y ajustes válidos.
    unsafe {
        let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(key(AVMediaTypeAudio)?, Some(&settings));
        input.setExpectsMediaDataInRealTime(true);
        Ok(input)
    }
}

impl Recording {
    /// Empieza a grabar. Devuelve también avisos (p. ej. micrófono no
    /// disponible) que no impiden grabar. `on_error` se llama si el sistema
    /// detiene la captura por su cuenta.
    pub fn start(opts: Options, on_error: impl Fn(String) + Send + Sync + 'static) -> Result<(Self, Vec<String>), String> {
        let mut warnings = Vec::new();
        let content = shareable_content()?;
        // SAFETY: llamadas a ScreenCaptureKit/AVFoundation con objetos válidos.
        unsafe {
            let config = SCStreamConfiguration::new();
            let (filter, width, height) = match &opts.source {
                Source::Display { id, rect, scale } => {
                    let display = content
                        .displays()
                        .iter()
                        .find(|d| d.displayID() == *id)
                        .ok_or("La pantalla ya no está conectada")?;
                    let own = std::process::id() as i32;
                    let ours: Vec<_> = content.applications().iter().filter(|a| a.processID() == own).collect();
                    let ours = objc2_foundation::NSArray::from_retained_slice(&ours);
                    let filter = SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                        SCContentFilter::alloc(),
                        &display,
                        &ours,
                        &objc2_foundation::NSArray::new(),
                    );
                    let (w, h) = match rect {
                        Some(r) => {
                            config.setSourceRect(CGRect::new(CGPoint::new(r.x, r.y), CGSize::new(r.width, r.height)));
                            (r.width * scale, r.height * scale)
                        }
                        None => (display.width() as f64 * scale, display.height() as f64 * scale),
                    };
                    (filter, w, h)
                }
                Source::Window { id } => {
                    let window = content
                        .windows()
                        .iter()
                        .find(|w| w.windowID() as u64 == *id)
                        .ok_or("La ventana ya no está abierta")?;
                    let filter = SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window);
                    let frame = window.frame();
                    // `pointPixelScale` existe desde macOS 14; antes, Retina.
                    let scale = if filter.respondsToSelector(sel!(pointPixelScale)) { filter.pointPixelScale() as f64 } else { 2.0 };
                    (filter, frame.size.width * scale, frame.size.height * scale)
                }
            };
            let (width, height) = output_size(width, height);
            config.setWidth(width);
            config.setHeight(height);
            config.setMinimumFrameInterval(CMTime::new(1, FPS));
            config.setPixelFormat(BGRA);
            config.setQueueDepth(6);
            config.setShowsCursor(true);
            if opts.system_audio {
                config.setCapturesAudio(true);
                config.setSampleRate(48_000);
                config.setChannelCount(2);
                config.setExcludesCurrentProcessAudio(true);
            }
            let microphone = opts.microphone && config.respondsToSelector(sel!(setCaptureMicrophone:));
            if opts.microphone && !microphone {
                warnings.push("Grabar el micrófono en los vídeos requiere macOS 15 o posterior".into());
            }
            if microphone {
                config.setCaptureMicrophone(true);
            }

            // Escritor MP4.
            let _ = std::fs::remove_file(&opts.path);
            let url = NSURL::from_file_path(&opts.path).ok_or("Ruta de grabación no válida")?;
            let writer = AVAssetWriter::assetWriterWithURL_fileType_error(&url, key(AVFileTypeMPEG4)?)
                .map_err(|e| format!("No se pudo crear el vídeo: {}", e.localizedDescription()))?;
            let compression = dict(&[
                (key(AVVideoAverageBitRateKey)?, number(video_bitrate(width, height) as f64)),
                (key(AVVideoMaxKeyFrameIntervalKey)?, number((FPS * 2) as f64)),
                (key(AVVideoExpectedSourceFrameRateKey)?, number(FPS as f64)),
                (key(AVVideoProfileLevelKey)?, string(key(AVVideoProfileLevelH264HighAutoLevel)?)),
            ]);
            let video_settings = dict(&[
                (key(AVVideoCodecKey)?, string(key(AVVideoCodecTypeH264)?)),
                (key(AVVideoWidthKey)?, number(width as f64)),
                (key(AVVideoHeightKey)?, number(height as f64)),
                (key(AVVideoCompressionPropertiesKey)?, any(compression)),
            ]);
            let video = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(key(AVMediaTypeVideo)?, Some(&video_settings));
            video.setExpectsMediaDataInRealTime(true);
            let system = if opts.system_audio { Some(audio_input(2)?) } else { None };
            let mic = if microphone { Some(audio_input(1)?) } else { None };
            for input in std::iter::once(&video).chain(system.iter()).chain(mic.iter()) {
                if !writer.canAddInput(input) {
                    return Err("El codificador de vídeo no admite esta configuración".into());
                }
                writer.addInput(input);
            }

            let shared = Arc::new(Shared {
                writer: Mutex::new(Writer { writer, video, system, mic, started: false, stopped: false }),
                on_error: Box::new(on_error),
            });
            let output = StreamOutput::new(shared);
            let stream = SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(ProtocolObject::from_ref(&*output)),
            );
            let queue = DispatchQueue::new("com.alex.screencut.record", DispatchQueueAttr::SERIAL);
            let kinds = [
                (true, SCStreamOutputType::Screen),
                (opts.system_audio, SCStreamOutputType::Audio),
                (microphone, SCStreamOutputType::Microphone),
            ];
            for (wanted, kind) in kinds {
                if wanted {
                    stream
                        .addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*output), kind, Some(&queue))
                        .map_err(|e| format!("No se pudo preparar la grabación: {}", e.localizedDescription()))?;
                }
            }
            wait_for(|block| stream.startCaptureWithCompletionHandler(Some(block)))
                .map_err(|e| format!("No se pudo empezar a grabar: {e}"))?;
            Ok((Self { stream, output, _queue: queue }, warnings))
        }
    }

    /// Detiene la captura y cierra el MP4.
    pub fn stop(self) -> Result<(), String> {
        // SAFETY: objetos válidos; los callbacks ya no escriben tras `stopped`.
        unsafe {
            let _ = wait_for(|block| self.stream.stopCaptureWithCompletionHandler(Some(block)));
            let shared = self.output.ivars().clone();
            let writer = {
                let mut w = shared.writer.lock().unwrap_or_else(|e| e.into_inner());
                w.stopped = true;
                if !w.started {
                    w.writer.cancelWriting();
                    return Err("No se grabó ningún fotograma".into());
                }
                // El último fotograma dura hasta ahora (con la pantalla quieta
                // no llegan fotogramas nuevos).
                w.writer.endSessionAtSourceTime(CMClock::host_time_clock().time());
                for input in std::iter::once(&w.video).chain(w.system.iter()).chain(w.mic.iter()) {
                    input.markAsFinished();
                }
                w.writer.clone()
            };
            let (tx, rx) = mpsc::channel();
            let block = RcBlock::new(move || {
                let _ = tx.send(());
            });
            writer.finishWritingWithCompletionHandler(&block);
            rx.recv_timeout(Duration::from_secs(30)).map_err(|_| "No se pudo terminar de escribir el vídeo")?;
            if writer.status() != AVAssetWriterStatus::Completed {
                let error = writer.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default();
                return Err(format!("El vídeo no se guardó bien: {error}"));
            }
            Ok(())
        }
    }
}
