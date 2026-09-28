//! Captura de audio con cpal. Cada fuente vive en su propio hilo porque
//! `cpal::Stream` no es `Send` en todas las plataformas.
//!
//! Audio del sistema:
//! - Windows: WASAPI loopback (stream de entrada sobre el dispositivo de salida).
//! - macOS 14.6+: "process tap" de Core Audio, igual desde cpal. Pide el
//!   permiso "Grabación de audio del sistema".
//! - Linux: la fuente `.monitor` de la salida por defecto (PulseAudio o PipeWire).

use super::PcmBlock;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub enum Source {
    Microphone,
    System,
}

/// Una captura en marcha. Se detiene con `stop` (o al soltarla).
pub struct AudioStream {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl AudioStream {
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.take(); // cerrar el canal despierta al hilo
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for AudioStream {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Empieza a capturar `source`; `on_block` recibe los bloques desde el hilo de audio.
pub fn start(source: Source, on_block: impl FnMut(PcmBlock) + Send + 'static) -> Result<AudioStream, String> {
    if matches!(source, Source::Microphone) {
        microphone_access()?;
    }
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        match open(source, on_block) {
            Ok(stream) => {
                let _ = ready_tx.send(Ok(()));
                let _ = stop_rx.recv(); // hasta que se cierre el canal
                drop(stream);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        }
    });
    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(AudioStream { stop: Some(stop_tx), thread: Some(thread) }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("El dispositivo de audio no respondió".into()),
    }
}

fn open(source: Source, on_block: impl FnMut(PcmBlock) + Send + 'static) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let (device, config) = match source {
        Source::Microphone => {
            let device = host.default_input_device().ok_or("No hay ningún micrófono")?;
            let config = device.default_input_config().map_err(|e| format!("Micrófono: {e}"))?;
            (device, config)
        }
        Source::System => system_device(&host)?,
    };
    let format = config.sample_format();
    let config: cpal::StreamConfig = config.into();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, config, on_block),
        SampleFormat::I16 => build::<i16>(&device, config, on_block),
        SampleFormat::I32 => build::<i32>(&device, config, on_block),
        SampleFormat::U16 => build::<u16>(&device, config, on_block),
        SampleFormat::F64 => build::<f64>(&device, config, on_block),
        SampleFormat::U8 => build::<u8>(&device, config, on_block),
        SampleFormat::I8 => build::<i8>(&device, config, on_block),
        other => return Err(format!("Formato de audio no soportado: {other}")),
    }
    .map_err(|e| describe(source, e))?;
    stream.play().map_err(|e| describe(source, e))?;
    Ok(stream)
}

/// macOS no da error al abrir el micrófono sin permiso: entrega silencio. Por
/// eso se comprueba antes y, la primera vez, se pide (espera a la respuesta).
#[cfg(target_os = "macos")]
pub fn microphone_access() -> Result<(), String> {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    const DENIED: &str = "ScreenCut no tiene permiso para usar el micrófono. Actívalo en Ajustes del Sistema → \
                          Privacidad y seguridad → Micrófono.";
    // SAFETY: consultas de clase de AVFoundation con un tipo de medio válido.
    unsafe {
        let Some(audio) = AVMediaTypeAudio else { return Ok(()) };
        match AVCaptureDevice::authorizationStatusForMediaType(audio) {
            AVAuthorizationStatus::Authorized => Ok(()),
            AVAuthorizationStatus::NotDetermined => {
                let (tx, rx) = mpsc::channel();
                let block = block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
                    let _ = tx.send(granted.as_bool());
                });
                AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &block);
                match rx.recv_timeout(Duration::from_secs(120)) {
                    Ok(true) => Ok(()),
                    _ => Err(DENIED.into()),
                }
            }
            _ => Err(DENIED.into()),
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn microphone_access() -> Result<(), String> {
    Ok(())
}

fn describe(source: Source, e: cpal::Error) -> String {
    match source {
        Source::Microphone => format!("No se pudo usar el micrófono: {e}"),
        Source::System => format!("No se pudo grabar el audio del sistema: {e}"),
    }
}

#[cfg(not(target_os = "linux"))]
fn system_device(host: &cpal::Host) -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    let device = host.default_output_device().ok_or("No hay ninguna salida de audio")?;
    let config = device.default_output_config().map_err(|e| format!("Salida de audio: {e}"))?;
    Ok((device, config))
}

#[cfg(target_os = "linux")]
fn system_device(host: &cpal::Host) -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    let sink = host.default_output_device().ok_or("No hay ninguna salida de audio")?;
    let sink_id = sink.id().map_err(|e| e.to_string())?;
    let monitor = format!("{}.monitor", sink_id.id());
    let device = host
        .input_devices()
        .map_err(|e| e.to_string())?
        .find(|d| d.id().map(|id| id.id() == monitor).unwrap_or(false))
        .ok_or("No se encontró el monitor de la salida de audio (hace falta PulseAudio o PipeWire)")?;
    let config = device.default_input_config().map_err(|e| format!("Monitor de audio: {e}"))?;
    Ok((device, config))
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut on_block: impl FnMut(PcmBlock) + Send + 'static,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let (rate, channels) = (config.sample_rate, config.channels);
    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _| {
            let frames = data.len() / channels.max(1) as usize;
            // El bloque acaba ahora: empezó hace lo que dura.
            let at = Instant::now() - Duration::from_secs_f64(frames as f64 / rate as f64);
            let samples = data.iter().map(|s| s.to_sample::<f32>()).collect();
            on_block(PcmBlock { at, rate, channels, samples });
        },
        |e| eprintln!("audio: {e}"),
        None,
    )
}
