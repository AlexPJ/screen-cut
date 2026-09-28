//! Grabación en macOS: ScreenCaptureKit entrega los fotogramas y el audio (del
//! sistema y, desde macOS 15, del micrófono) y AVAssetWriter los codifica a
//! MP4 por hardware. ScreenCut se excluye del filtro, así que sus ventanas
//! (el control de grabación incluido) nunca salen en el vídeo. El micrófono
//! llega como otra pista de audio; al terminar se mezcla con la del sistema en
//! una sola, porque muchos reproductores solo reproducen la primera.

use super::{output_size, video_bitrate, Options, Source};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, AllocAnyThread, DefinedClass};
use objc2_av_foundation::{
    AVAssetReader, AVAssetReaderAudioMixOutput, AVAssetReaderStatus, AVAssetReaderTrackOutput, AVURLAsset, AVAssetWriter, AVAssetWriterInput, AVAssetWriterStatus, AVFileTypeMPEG4, AVMediaTypeAudio, AVMediaTypeVideo,
    AVVideoAverageBitRateKey, AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoCompressionPropertiesKey,
    AVVideoExpectedSourceFrameRateKey, AVVideoHeightKey, AVVideoMaxKeyFrameIntervalKey, AVVideoProfileLevelH264HighAutoLevel,
    AVVideoProfileLevelKey, AVVideoWidthKey,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_media::{kCMTimeZero, CMClock, CMSampleBuffer, CMTime};
use objc2_foundation::{NSCopying, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSString, NSURL};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamOutput,
    SCStreamOutputType,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FPS: i32 = 30;
/// `kCVPixelFormatType_32BGRA`.
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
/// `kAudioFormatMPEG4AAC`.
const AAC: u32 = u32::from_be_bytes(*b"aac ");
/// `kAudioFormatLinearPCM`.
const LPCM: u32 = u32::from_be_bytes(*b"lpcm");

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
    path: PathBuf,
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

fn audio_input(channels: u32, realtime: bool) -> Result<Retained<AVAssetWriterInput>, String> {
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
        input.setExpectsMediaDataInRealTime(realtime);
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
                Source::Display { screen, rect } => {
                    let display = content
                        .displays()
                        .iter()
                        .find(|d| d.displayID() == screen.id)
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
                    // ScreenCaptureKit recorta en puntos.
                    let (w, h) = match rect {
                        Some(r) => {
                            let s = screen.scale;
                            config.setSourceRect(CGRect::new(
                                CGPoint::new(r.x / s, r.y / s),
                                CGSize::new(r.width / s, r.height / s),
                            ));
                            (r.width, r.height)
                        }
                        None => (display.width() as f64 * screen.scale, display.height() as f64 * screen.scale),
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
            let mut microphone = opts.microphone && config.respondsToSelector(sel!(setCaptureMicrophone:));
            if opts.microphone && !microphone {
                warnings.push("Grabar el micrófono en los vídeos requiere macOS 15 o posterior".into());
            }
            if microphone {
                if let Err(e) = crate::infra::audio::device::microphone_access() {
                    warnings.push(e);
                    microphone = false;
                }
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
            let system = if opts.system_audio { Some(audio_input(2, true)?) } else { None };
            let mic = if microphone { Some(audio_input(1, true)?) } else { None };
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
            Ok((Self { stream, output, _queue: queue, path: opts.path }, warnings))
        }
    }

    /// Detiene la captura y cierra el MP4.
    pub fn stop(self) -> Result<(), String> {
        // SAFETY: objetos válidos; los callbacks ya no escriben tras `stopped`.
        unsafe {
            let _ = wait_for(|block| self.stream.stopCaptureWithCompletionHandler(Some(block)));
            let shared = self.output.ivars().clone();
            let (writer, two_tracks) = {
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
                (w.writer.clone(), w.system.is_some() && w.mic.is_some())
            };
            finish(&writer)?;
            if two_tracks {
                // Si falla, se queda el vídeo con las dos pistas por separado.
                if let Err(e) = mix_audio_tracks(&self.path) {
                    eprintln!("grabación: no se pudo mezclar el micrófono: {e}");
                }
            }
            Ok(())
        }
    }
}

/// Cierra el archivo y espera a que AVAssetWriter termine.
unsafe fn finish(writer: &AVAssetWriter) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move || {
        let _ = tx.send(());
    });
    writer.finishWritingWithCompletionHandler(&block);
    rx.recv_timeout(Duration::from_secs(60)).map_err(|_| "No se pudo terminar de escribir el vídeo")?;
    if writer.status() != AVAssetWriterStatus::Completed {
        let error = writer.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default();
        return Err(format!("El vídeo no se guardó bien: {error}"));
    }
    Ok(())
}

/// Reescribe el MP4 con todas sus pistas de audio mezcladas en una sola. El
/// vídeo se copia tal cual (sin recodificar); solo se codifica el audio, así
/// que tarda unos segundos incluso con grabaciones largas.
fn mix_audio_tracks(path: &Path) -> Result<(), String> {
    let tmp = path.with_extension("mezcla.mp4");
    let _ = std::fs::remove_file(&tmp);
    let result = unsafe { remux(path, &tmp) };
    match result {
        Ok(()) => std::fs::rename(&tmp, path).map_err(|e| e.to_string()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

// Las variantes síncronas están obsoletas, pero esto ya corre fuera del hilo
// principal y así se lee mucho más simple.
#[allow(deprecated)]
unsafe fn remux(from: &Path, to: &Path) -> Result<(), String> {
    let describe = |e: Retained<NSError>| e.localizedDescription().to_string();
    let url = NSURL::from_file_path(from).ok_or("Ruta no válida")?;
    let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
    let video_tracks = asset.tracksWithMediaType(key(AVMediaTypeVideo)?);
    let audio_tracks = asset.tracksWithMediaType(key(AVMediaTypeAudio)?);
    let video_track = video_tracks.firstObject().ok_or("El vídeo no tiene imagen")?;
    if audio_tracks.count() < 2 {
        return Ok(());
    }

    let reader = AVAssetReader::assetReaderWithAsset_error(&asset).map_err(describe)?;
    let video_out = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(&video_track, None);
    video_out.setAlwaysCopiesSampleData(false);
    let pcm = dict(&[
        (&NSString::from_str("AVFormatIDKey"), number(LPCM as f64)),
        (&NSString::from_str("AVSampleRateKey"), number(48_000.0)),
        (&NSString::from_str("AVNumberOfChannelsKey"), number(2.0)),
        (&NSString::from_str("AVLinearPCMBitDepthKey"), number(16.0)),
        (&NSString::from_str("AVLinearPCMIsFloatKey"), any(NSNumber::new_bool(false))),
        (&NSString::from_str("AVLinearPCMIsBigEndianKey"), any(NSNumber::new_bool(false))),
        (&NSString::from_str("AVLinearPCMIsNonInterleaved"), any(NSNumber::new_bool(false))),
    ]);
    let audio_out = AVAssetReaderAudioMixOutput::assetReaderAudioMixOutputWithAudioTracks_audioSettings(&audio_tracks, Some(&pcm));
    for output in [&**video_out, &**audio_out] {
        if !reader.canAddOutput(output) {
            return Err("No se puede leer el vídeo".into());
        }
        reader.addOutput(output);
    }
    if !reader.startReading() {
        return Err(reader.error().map(describe).unwrap_or_default());
    }

    // El formato del vídeo, necesario para copiarlo sin recodificar.
    let formats = video_track.formatDescriptions();
    let format = formats.firstObject().ok_or("El vídeo no tiene formato")?;
    // SAFETY: los elementos de `formatDescriptions` son CMFormatDescription.
    let hint = &*(Retained::as_ptr(&format) as *const objc2_core_media::CMFormatDescription);
    let out_url = NSURL::from_file_path(to).ok_or("Ruta no válida")?;
    let writer = AVAssetWriter::assetWriterWithURL_fileType_error(&out_url, key(AVFileTypeMPEG4)?).map_err(describe)?;
    let video_in = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings_sourceFormatHint(
        key(AVMediaTypeVideo)?,
        None,
        Some(hint),
    );
    video_in.setExpectsMediaDataInRealTime(false);
    let audio_in = audio_input(2, false)?;
    for input in [&video_in, &audio_in] {
        if !writer.canAddInput(input) {
            return Err("No se puede escribir el vídeo mezclado".into());
        }
        writer.addInput(input);
    }
    if !writer.startWriting() {
        return Err(writer.error().map(describe).unwrap_or_default());
    }
    writer.startSessionAtSourceTime(kCMTimeZero);

    // AVAssetWriter intercala las pistas: cada entrada acepta datos solo
    // cuando la otra no se ha quedado atrás.
    let (mut video_done, mut audio_done) = (false, false);
    while !(video_done && audio_done) {
        let mut progressed = false;
        if !video_done && video_in.isReadyForMoreMediaData() {
            match video_out.copyNextSampleBuffer() {
                Some(sample) => {
                    video_in.appendSampleBuffer(&sample);
                }
                None => {
                    video_in.markAsFinished();
                    video_done = true;
                }
            }
            progressed = true;
        }
        if !audio_done && audio_in.isReadyForMoreMediaData() {
            match audio_out.copyNextSampleBuffer() {
                Some(sample) => {
                    audio_in.appendSampleBuffer(&sample);
                }
                None => {
                    audio_in.markAsFinished();
                    audio_done = true;
                }
            }
            progressed = true;
        }
        if writer.status() == AVAssetWriterStatus::Failed {
            reader.cancelReading();
            return Err(writer.error().map(describe).unwrap_or_default());
        }
        if !progressed {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    if reader.status() == AVAssetReaderStatus::Failed {
        writer.cancelWriting();
        return Err(reader.error().map(describe).unwrap_or_default());
    }
    writer.endSessionAtSourceTime(asset.duration());
    finish(&writer)
}
