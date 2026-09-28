//! Grabación en Linux con GStreamer. En X11 se captura con `ximagesrc`; en
//! Wayland hay que pasar por el portal ScreenCast (el usuario elige la pantalla
//! o ventana en el diálogo del sistema) y leer el vídeo de PipeWire. El audio
//! sale de PulseAudio/PipeWire: el monitor de la salida por defecto y el
//! micrófono, mezclados en una pista. Los codificadores dependen de los plugins
//! instalados: MP4 (H.264 + AAC) si los hay y, si no, WebM (VP8 + Opus), que
//! solo necesita plugins-base y plugins-good.

use super::{output_size, video_bitrate, Options, Source};
use gst::prelude::*;
use gstreamer as gst;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

pub struct Recording {
    pipeline: gst::Pipeline,
    path: PathBuf,
    /// Fin del vídeo (EOS) o error, según lo ve el hilo del bus.
    ended: mpsc::Receiver<Result<(), String>>,
    stopping: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    bus_thread: Option<JoinHandle<()>>,
    /// La sesión del portal (Wayland) tiene que seguir abierta mientras se graba.
    portal: Option<Portal>,
}

/// Un codificador y cómo decirle el bitrate (que depende del tamaño final,
/// que en Wayland no se sabe hasta que llega el primer fotograma).
struct VideoEncoder {
    factory: &'static str,
    /// Formato de entrada: I420 para no acabar en perfiles 4:4:4 que no abre
    /// casi ningún reproductor.
    desc: &'static str,
    bitrate: &'static str,
    /// Bits por segundo de cada unidad de `bitrate`.
    unit: usize,
    signed: bool,
}

const H264: &[VideoEncoder] = &[
    // Por hardware (VA-API): solo existe si hay una GPU que codifique.
    VideoEncoder {
        factory: "vah264enc",
        desc: "video/x-raw,format=NV12 ! vah264enc name=venc key-int-max=60 ! h264parse",
        bitrate: "bitrate",
        unit: 1000,
        signed: false,
    },
    VideoEncoder {
        factory: "x264enc",
        desc: "video/x-raw,format=I420 ! x264enc name=venc tune=zerolatency speed-preset=veryfast key-int-max=60 \
               ! video/x-h264,profile=high ! h264parse",
        bitrate: "bitrate",
        unit: 1000,
        signed: false,
    },
    VideoEncoder {
        factory: "openh264enc",
        desc: "video/x-raw,format=I420 ! openh264enc name=venc gop-size=60 ! h264parse",
        bitrate: "bitrate",
        unit: 1,
        signed: false,
    },
];

const VP8: VideoEncoder = VideoEncoder {
    factory: "vp8enc",
    desc: "video/x-raw,format=I420 ! vp8enc name=venc deadline=1 cpu-used=8 end-usage=cbr keyframe-max-dist=60 threads=4",
    bitrate: "target-bitrate",
    unit: 1,
    signed: true,
};

const AAC: &[&str] = &["fdkaacenc", "avenc_aac", "voaacenc"];

/// Formato elegido según los plugins disponibles.
struct Format {
    video: &'static VideoEncoder,
    /// Codificador de audio (con sus opciones), si hay audio.
    audio: Option<String>,
    /// Muxer y extensión.
    mux: &'static str,
    extension: &'static str,
}

fn available(factory: &str) -> bool {
    gst::ElementFactory::find(factory).is_some()
}

fn choose_format(audio: bool, warnings: &mut Vec<String>) -> Result<Format, String> {
    let h264 = H264.iter().find(|e| available(e.factory));
    let aac = AAC.iter().find(|f| available(f));
    if let Some(video) = h264 {
        if !audio || aac.is_some() {
            return Ok(Format {
                video,
                audio: aac.map(|f| format!("{f} bitrate=128000 ! aacparse")),
                // Fragmentado: si la grabación se corta de golpe, lo grabado
                // hasta ahí sigue siendo reproducible.
                mux: "mp4mux name=mux fragment-duration=2000",
                extension: "mp4",
            });
        }
    }
    if available(VP8.factory) && available("webmmux") && (!audio || available("opusenc")) {
        warnings.push(
            "Faltan los codificadores de MP4 de GStreamer: el vídeo se guarda en WebM. \
             Para MP4, instala gstreamer1.0-plugins-ugly y gstreamer1.0-libav."
                .into(),
        );
        return Ok(Format {
            video: &VP8,
            audio: audio.then(|| "opusenc bitrate=128000".into()),
            mux: "webmmux name=mux",
            extension: "webm",
        });
    }
    Err("Faltan plugins de GStreamer para grabar vídeo. Instala gstreamer1.0-plugins-good \
         (y, para MP4, gstreamer1.0-plugins-ugly y gstreamer1.0-libav)."
        .into())
}

fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland"))
}

/// Recorte a aplicar sobre los fotogramas de la fuente, en fracciones de su
/// tamaño (en Wayland no se sabe el tamaño real hasta que llegan).
#[derive(Clone, Copy, Default)]
struct Crop {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Recording {
    pub fn start(opts: Options, on_error: impl Fn(String) + Send + Sync + 'static) -> Result<(Self, Vec<String>), String> {
        gst::init().map_err(|e| format!("No se pudo iniciar GStreamer: {e}"))?;
        let mut warnings = Vec::new();
        let want_audio = opts.system_audio || opts.microphone;
        let format = choose_format(want_audio, &mut warnings)?;
        let path = opts.path.with_extension(format.extension);
        let portal = if is_wayland() {
            Some(Portal::open(matches!(opts.source, Source::Window { .. }))?)
        } else {
            None
        };
        let on_error: Arc<dyn Fn(String) + Send + Sync> = Arc::new(on_error);

        let started = match launch(&opts, &format, format.audio.as_deref(), &path, portal.as_ref()) {
            Err(e) if format.audio.is_some() => {
                // Sin servidor de sonido (o sin micrófono) se graba al menos la imagen.
                let started = launch(&opts, &format, None, &path, portal.as_ref())?;
                warnings.push(format!("No se pudo grabar el audio: {e}"));
                started
            }
            other => other?,
        };
        let pipeline = started;

        let (tx, ended) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let bus = pipeline.bus().ok_or("La grabación no tiene bus de mensajes")?;
        let bus_thread = std::thread::spawn({
            let (stopping, done) = (stopping.clone(), done.clone());
            move || {
                while !done.load(Ordering::SeqCst) {
                    let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(200)) else { continue };
                    match msg.view() {
                        gst::MessageView::Eos(_) => {
                            let _ = tx.send(Ok(()));
                            return;
                        }
                        gst::MessageView::Error(err) => {
                            let text = err.error().to_string();
                            eprintln!("grabación: {text} ({:?})", err.debug());
                            // Primero se avisa a `stop`, que on_error acaba llamando.
                            let _ = tx.send(Err(text.clone()));
                            if !stopping.load(Ordering::SeqCst) {
                                on_error(format!("La grabación se ha interrumpido: {text}"));
                            }
                            return;
                        }
                        _ => {}
                    }
                }
            }
        });
        Ok((Self { pipeline, path, ended, stopping, done, bus_thread: Some(bus_thread), portal }, warnings))
    }

    /// Dónde se guarda el vídeo (la extensión depende del formato elegido).
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn stop(mut self) -> Result<(), String> {
        self.stopping.store(true, Ordering::SeqCst);
        // Si ya hubo un error, el muxer no recibirá el fin de flujo; el MP4 es
        // fragmentado, así que lo grabado hasta entonces se puede reproducir.
        let result = match self.ended.try_recv() {
            Ok(_) => Ok(()),
            Err(_) => {
                self.pipeline.send_event(gst::event::Eos::new());
                match self.ended.recv_timeout(Duration::from_secs(10)) {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(format!("No se pudo terminar el vídeo: {e}")),
                    Err(_) => Err("El vídeo no terminó de guardarse a tiempo".into()),
                }
            }
        };
        self.shutdown();
        result
    }

    fn shutdown(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        self.done.store(true, Ordering::SeqCst);
        if let Some(t) = self.bus_thread.take() {
            let _ = t.join();
        }
        if let Some(portal) = self.portal.take() {
            portal.close();
        }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        if self.bus_thread.is_some() {
            self.shutdown();
        }
    }
}

/// Monta la tubería y la pone en marcha. Devuelve el error del bus si algún
/// elemento no arranca (p. ej. no hay servidor de sonido).
fn launch(
    opts: &Options,
    format: &Format,
    audio: Option<&str>,
    path: &Path,
    portal: Option<&Portal>,
) -> Result<gst::Pipeline, String> {
    let (source, crop) = video_source(opts, portal)?;
    let mut desc = format!(
        "{source} ! videoconvert ! videocrop name=crop ! videoscale add-borders=true ! capsfilter name=size \
         ! videoconvert ! {venc} ! queue ! {mux} ! filesink name=sink",
        venc = format.video.desc,
        mux = format.mux,
    );
    if let Some(aenc) = audio {
        desc.push_str(&format!(" audiomixer name=amix ! audioconvert ! audioresample ! {aenc} ! queue ! mux."));
        let branch = |name: &str| {
            format!(
                " pulsesrc name={name} provide-clock=false ! audioconvert ! audioresample \
                 ! audio/x-raw,rate=48000,channels=2 ! queue ! amix."
            )
        };
        if opts.system_audio {
            desc.push_str(&branch("sys"));
        }
        if opts.microphone {
            desc.push_str(&branch("mic"));
        }
    }
    let pipeline = gst::parse::launch(&desc)
        .map_err(|e| format!("No se pudo preparar la grabación: {e}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "No se pudo preparar la grabación")?;
    let by_name = |name: &str| pipeline.by_name(name).ok_or_else(|| format!("Falta el elemento {name}"));

    by_name("sink")?.set_property("location", path.to_string_lossy().as_ref());
    if let Some(sys) = pipeline.by_name("sys") {
        // Nombre especial de PulseAudio (y de pipewire-pulse): el monitor de
        // la salida de audio por defecto, es decir, lo que suena.
        sys.set_property("device", "@DEFAULT_MONITOR@");
    }
    match (&opts.source, portal) {
        (_, Some(portal)) => {
            let src = by_name("vsrc")?;
            src.set_property("fd", portal.fd.as_raw_fd());
            src.set_property("path", portal.node.to_string());
        }
        (Source::Display { screen, rect }, None) => {
            // En X11 las coordenadas del monitor y de la región son píxeles
            // del escritorio.
            let (x, y, w, h) = match rect {
                Some(r) => (screen.x as f64 + r.x, screen.y as f64 + r.y, r.width, r.height),
                None => (screen.x as f64, screen.y as f64, screen.width as f64, screen.height as f64),
            };
            let (x, y) = (x.max(0.0).round() as u32, y.max(0.0).round() as u32);
            let (w, h) = (w.round().max(2.0) as u32, h.round().max(2.0) as u32);
            let src = by_name("vsrc")?;
            src.set_property("startx", x);
            src.set_property("starty", y);
            src.set_property("endx", x + w - 1);
            src.set_property("endy", y + h - 1);
        }
        (Source::Window { id }, None) => by_name("vsrc")?.set_property("xid", *id),
    }
    size_on_first_frame(&by_name("crop")?, by_name("size")?, by_name("venc")?, format.video, crop);

    if let Err(e) = pipeline.set_state(gst::State::Playing) {
        let detail = bus_error(&pipeline, Duration::ZERO).unwrap_or_else(|| e.to_string());
        let _ = pipeline.set_state(gst::State::Null);
        return Err(detail);
    }
    // Algunos fallos (negociar el formato con PipeWire…) llegan un instante
    // después de arrancar.
    if let Some(detail) = bus_error(&pipeline, Duration::from_millis(500)) {
        let _ = pipeline.set_state(gst::State::Null);
        return Err(detail);
    }
    Ok(pipeline)
}

/// El primer error pendiente en el bus, esperando como mucho `wait`.
fn bus_error(pipeline: &gst::Pipeline, wait: Duration) -> Option<String> {
    let bus = pipeline.bus()?;
    let msg = bus.timed_pop_filtered(
        gst::ClockTime::from_mseconds(wait.as_millis() as u64),
        &[gst::MessageType::Error],
    )?;
    match msg.view() {
        gst::MessageView::Error(err) => {
            eprintln!("grabación: {} ({:?})", err.error(), err.debug());
            Some(err.error().to_string())
        }
        _ => None,
    }
}

/// La fuente de vídeo (siempre llamada `vsrc`), a 30 fps, y el recorte que
/// hay que aplicarle.
fn video_source(opts: &Options, portal: Option<&Portal>) -> Result<(String, Crop), String> {
    let x11 = "ximagesrc name=vsrc use-damage=false show-pointer=true ! video/x-raw,framerate=30/1";
    let Some(portal) = portal else {
        return Ok((x11.into(), Crop::default()));
    };
    // PipeWire solo manda fotogramas cuando cambia algo: videorate los repite.
    let src = "pipewiresrc name=vsrc do-timestamp=true keepalive-time=1000 ! videorate ! video/x-raw,framerate=30/1";
    let crop = match &opts.source {
        // La región va en píxeles de la captura del monitor; el portal deja
        // elegir el monitor en su diálogo, así que se recorta en proporción.
        Source::Display { screen, rect: Some(r) } => {
            let (w, h) = (screen.width as f64 * screen.scale, screen.height as f64 * screen.scale);
            if let Some((sw, sh)) = portal.size {
                if (sw as f64 / sh as f64 - w / h).abs() > 0.02 {
                    eprintln!("grabación: el monitor elegido en el portal ({sw}×{sh}) no es el de la región");
                }
            }
            Crop {
                left: (r.x / w).clamp(0.0, 1.0),
                top: (r.y / h).clamp(0.0, 1.0),
                right: (1.0 - (r.x + r.width) / w).clamp(0.0, 1.0),
                bottom: (1.0 - (r.y + r.height) / h).clamp(0.0, 1.0),
            }
        }
        _ => Crop::default(),
    };
    Ok((src.into(), crop))
}

/// Cuando se conoce el tamaño de los fotogramas (el primer evento de caps),
/// fija el recorte, el tamaño de salida (par y como mucho 4K) y el bitrate.
/// Solo la primera vez: si la ventana cambia de tamaño, `videoscale` la
/// encaja con bandas en el mismo tamaño de vídeo.
fn size_on_first_frame(
    crop: &gst::Element,
    size: gst::Element,
    encoder: gst::Element,
    spec: &'static VideoEncoder,
    fractions: Crop,
) {
    let Some(pad) = crop.static_pad("sink") else { return };
    let crop = crop.clone();
    let configured = AtomicBool::new(false);
    pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
        let Some(gst::EventView::Caps(caps)) = info.event().map(|e| e.view()) else {
            return gst::PadProbeReturn::Ok;
        };
        let Some(s) = caps.caps().structure(0) else { return gst::PadProbeReturn::Ok };
        let (Ok(w), Ok(h)) = (s.get::<i32>("width"), s.get::<i32>("height")) else {
            return gst::PadProbeReturn::Ok;
        };
        if configured.swap(true, Ordering::SeqCst) {
            return gst::PadProbeReturn::Ok;
        }
        let px = |f: f64, total: i32| (f * total as f64).round() as i32;
        let (left, top) = (px(fractions.left, w), px(fractions.top, h));
        let (mut right, mut bottom) = (px(fractions.right, w), px(fractions.bottom, h));
        // H.264 necesita dimensiones pares.
        right += (w - left - right).rem_euclid(2);
        bottom += (h - top - bottom).rem_euclid(2);
        let (cw, ch) = ((w - left - right).max(2), (h - top - bottom).max(2));
        crop.set_property("left", left);
        crop.set_property("top", top);
        crop.set_property("right", right);
        crop.set_property("bottom", bottom);

        let (ow, oh) = output_size(cw as f64, ch as f64);
        size.set_property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("width", ow as i32)
                .field("height", oh as i32)
                .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                .build(),
        );
        let bitrate = video_bitrate(ow, oh) / spec.unit;
        if spec.signed {
            encoder.set_property(spec.bitrate, bitrate as i32);
        } else {
            encoder.set_property(spec.bitrate, bitrate as u32);
        }
        gst::PadProbeReturn::Ok
    });
}

/// Sesión del portal ScreenCast. Vive en su propio hilo con un runtime de
/// tokio que atiende la conexión D-Bus hasta que se cierra.
struct Portal {
    fd: OwnedFd,
    node: u32,
    size: Option<(i32, i32)>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Portal {
    fn open(window: bool) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let (stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            };
            runtime.block_on(async move {
                match select_source(window).await {
                    Ok((proxy, session, stream, fd)) => {
                        let _ = tx.send(Ok((stream, fd)));
                        let _ = stop_rx.await;
                        let _ = session.close().await;
                        drop(proxy);
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            });
        });
        // Espera a que el usuario elija en el diálogo del sistema.
        match rx.recv() {
            Ok(Ok((stream, fd))) => Ok(Self {
                fd,
                node: stream.pipe_wire_node_id(),
                size: stream.size(),
                stop: Some(stop),
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("El portal de captura de pantalla se cerró inesperadamente".into()),
        }
    }

    fn close(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

async fn select_source(
    window: bool,
) -> Result<
    (
        ashpd::desktop::screencast::Screencast,
        ashpd::desktop::Session<ashpd::desktop::screencast::Screencast>,
        ashpd::desktop::screencast::Stream,
        OwnedFd,
    ),
    String,
> {
    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    let err = |e: ashpd::Error| match e {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => "Se canceló la grabación".to_string(),
        e => format!("El portal de captura de pantalla falló: {e}"),
    };
    let proxy = Screencast::new()
        .await
        .map_err(|e| format!("No hay portal de captura de pantalla (xdg-desktop-portal): {e}"))?;
    let session = proxy.create_session(Default::default()).await.map_err(err)?;
    let mut options = SelectSourcesOptions::default()
        .set_sources(ashpd::enumflags2::BitFlags::from(if window { SourceType::Window } else { SourceType::Monitor }))
        .set_multiple(false);
    if proxy.available_cursor_modes().await.is_ok_and(|m| m.contains(CursorMode::Embedded)) {
        options = options.set_cursor_mode(CursorMode::Embedded);
    }
    proxy.select_sources(&session, options).await.map_err(err)?.response().map_err(err)?;
    let streams = proxy.start(&session, None, Default::default()).await.map_err(err)?.response().map_err(err)?;
    let stream = streams.streams().first().cloned().ok_or("No se eligió ninguna pantalla")?;
    let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await.map_err(err)?;
    Ok((proxy, session, stream, fd))
}
