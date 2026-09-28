//! Transcripción de una sesión a partir de sus WAV. Durante la sesión va
//! leyendo lo que se graba (con unos segundos de retraso); al terminar, o al
//! volver a transcribir, procesa todo el audio de una vez.

use super::model::{Segment, TranscriptInfo, TranscriptStatus};
use super::transcript::{self, SPEAKER_ME, SPEAKER_OTHERS};
use super::{export, store};
use crate::app::activity::Activity;
use crate::app::state::AppState;
use crate::infra::audio::{available_samples, read_samples};
use crate::infra::stt::{self, chunker};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

/// Pistas de audio de una sesión y a quién corresponden.
pub const TRACKS: [(&str, &str); 2] = [(SPEAKER_ME, "audio/mic.wav"), (SPEAKER_OTHERS, "audio/system.wav")];

pub struct Job {
    pub id: String,
    pub dir: PathBuf,
    pub language: String,
    pub model_id: &'static str,
    pub model: PathBuf,
    pub vad: Option<PathBuf>,
    /// `true` mientras la sesión sigue grabando: hay que esperar más audio.
    pub recording: Arc<AtomicBool>,
}

/// Sesiones transcribiéndose ahora mismo.
static RUNNING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn running<R>(f: impl FnOnce(&mut HashSet<String>) -> R) -> R {
    f(RUNNING.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new))
}

pub fn is_running(id: &str) -> bool {
    running(|r| r.contains(id))
}

#[derive(Serialize, Clone)]
struct TranscriptEvent<'a> {
    id: &'a str,
    status: TranscriptStatus,
    /// Audio ya transcrito y total grabado, en ms.
    done_ms: u64,
    total_ms: u64,
    error: Option<String>,
}

pub fn info(job: &Job, status: TranscriptStatus) -> TranscriptInfo {
    TranscriptInfo { status, language: job.language.clone(), model: Some(job.model_id.to_string()), error: None }
}

/// Lanza la transcripción en su propio hilo.
pub fn spawn(app: &AppHandle, job: Job) -> Result<(), String> {
    if !running(|r| r.insert(job.id.clone())) {
        return Err("Esa sesión ya se está transcribiendo".into());
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let result = run(
            &job,
            |segments| deliver(&app, &job, segments),
            |done_ms, total_ms| {
                let status = TranscriptStatus::Running;
                let _ = app.emit("session-transcript", TranscriptEvent { id: &job.id, status, done_ms, total_ms, error: None });
            },
        );
        if running(|r| {
            r.remove(&job.id);
            r.is_empty()
        }) {
            stt::unload(); // cientos de MB que no hacen falta hasta la próxima
        }
        let error = result.as_ref().err().cloned();
        let status = if error.is_some() { TranscriptStatus::Failed } else { TranscriptStatus::Done };
        if let Err(e) = finish(&app, &job, status, error.clone()) {
            let _ = app.emit("capture-error", e);
        }
        let _ = app.emit(
            "session-transcript",
            TranscriptEvent { id: &job.id, status, done_ms: 0, total_ms: 0, error },
        );
    });
    Ok(())
}

/// Recorre las pistas trozo a trozo hasta que la sesión deja de grabar y no
/// queda audio. `deliver` recibe los fragmentos nuevos y `progress` el audio
/// transcrito y el total grabado (en ms).
fn run(
    job: &Job,
    mut deliver: impl FnMut(Vec<Segment>) -> Result<(), String>,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    let tracks: Vec<(&str, PathBuf)> = TRACKS.iter().map(|(who, file)| (*who, job.dir.join(file))).collect();
    let mut offsets = vec![0u64; tracks.len()];
    let mut done = vec![false; tracks.len()];
    // Un poco más de 30 s: lo máximo que se corta de una vez.
    let window = chunker::RATE * 32;
    while done.iter().any(|d| !d) {
        // Se mira antes de leer: si ya no graba, lo que hay en disco es todo.
        let finished = !job.recording.load(Ordering::Acquire);
        let mut worked = false;
        for (i, (speaker, path)) in tracks.iter().enumerate() {
            if done[i] {
                continue;
            }
            let pending = if path.exists() {
                read_samples(path, offsets[i], window).map_err(|e| format!("No se pudo leer el audio: {e}"))?
            } else {
                Vec::new()
            };
            let Some(cut) = chunker::cut_point(&pending, finished) else {
                done[i] = finished;
                continue;
            };
            let lead = chunker::leading_silence(&pending[..cut]);
            let chunk = &pending[lead..cut];
            let start_ms = ms(offsets[i] + lead as u64);
            if !chunker::is_silent(chunk) {
                let pieces = stt::transcribe(&job.model, job.vad.as_deref(), &job.language, chunk)?;
                let segments: Vec<Segment> = pieces
                    .into_iter()
                    .map(|p| Segment {
                        start_ms: start_ms + p.start_ms,
                        end_ms: start_ms + p.end_ms.max(p.start_ms),
                        speaker: speaker.to_string(),
                        lang: p.lang,
                        text: p.text,
                    })
                    .collect();
                deliver(segments)?;
            }
            offsets[i] += cut as u64;
            worked = true;
            let total = tracks.iter().filter_map(|(_, p)| available_samples(p).ok()).max().unwrap_or(0);
            // Lo transcrito es lo que llevan todas las pistas que siguen activas.
            let transcribed = (0..tracks.len())
                .filter(|&t| !done[t] && tracks[t].1.exists())
                .map(|t| offsets[t])
                .min()
                .unwrap_or(total);
            progress(ms(transcribed), ms(total));
        }
        if !worked && done.iter().any(|d| !d) {
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    Ok(())
}

fn ms(samples: u64) -> u64 {
    samples * 1000 / chunker::RATE as u64
}

/// Guarda fragmentos nuevos: en la sesión en memoria si sigue en curso (la
/// que se escribe con cada captura) o directamente en `session.json`.
fn deliver(app: &AppHandle, job: &Job, segments: Vec<Segment>) -> Result<(), String> {
    if segments.is_empty() {
        return Ok(());
    }
    let state: State<AppState> = app.state();
    {
        let mut activity = state.activity.lock().unwrap();
        if let Activity::Session(s) = &mut *activity {
            if s.session.id == job.id {
                s.session.segments.extend(segments);
                return store::write(&s.dir, &s.session);
            }
        }
    }
    store::update(&job.dir, |s| s.segments.extend(segments)).map(|_| ())
}

/// Deja la transcripción limpia y ordenada, regenera los exportes y, si así se
/// ha configurado, borra el audio.
fn finish(app: &AppHandle, job: &Job, status: TranscriptStatus, error: Option<String>) -> Result<(), String> {
    let keep_audio = {
        let state: State<AppState> = app.state();
        let keep = state.settings.lock().unwrap().keep_session_audio;
        keep
    };
    let delete_audio = status == TranscriptStatus::Done && !keep_audio;
    let (session, _) = store::update(&job.dir, |s| {
        s.segments = transcript::clean(std::mem::take(&mut s.segments));
        s.transcript = Some(TranscriptInfo { error: error.clone(), ..info(job, status) });
        if delete_audio {
            s.audio.clear();
        }
    })?;
    if delete_audio {
        remove_audio(&job.dir);
    }
    export::write_outputs(&job.dir, &session)
}

fn remove_audio(dir: &Path) {
    for (_, file) in TRACKS {
        let _ = std::fs::remove_file(dir.join(file));
    }
    let _ = std::fs::remove_dir(dir.join("audio"));
}

/// ¿Tiene la sesión audio que transcribir?
pub fn has_audio(dir: &Path) -> bool {
    TRACKS.iter().any(|(_, file)| dir.join(file).exists())
}

#[cfg(test)]
mod probe {
    use super::*;
    use crate::infra::audio::wav::WavWriter;

    fn pcm(path: &Path) -> Vec<f32> {
        let b = std::fs::read(path).unwrap();
        let at = b.windows(4).position(|w| w == b"data").unwrap() + 8;
        b[at..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32767.0).collect()
    }

    /// Simula una sesión en directo: el WAV crece mientras se transcribe.
    /// `SCREENCUT_MODELS=… SCREENCUT_AUDIO=… cargo test probe_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_live() {
        let models = PathBuf::from(std::env::var("SCREENCUT_MODELS").unwrap());
        let audio = PathBuf::from(std::env::var("SCREENCUT_AUDIO").unwrap());
        let dir = std::env::temp_dir().join(format!("screencut-live-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("audio")).unwrap();
        let (es, en) = (pcm(&audio.join("es.wav")), pcm(&audio.join("en.wav")));
        // 3 s de silencio, frase, 20 s, frase, 12 s, frase en inglés, 4 s.
        let mut script = vec![0.0; 3 * 16_000];
        let mut starts = vec![3_000u64];
        script.extend(&es);
        script.extend(vec![0.0; 20 * 16_000]);
        starts.push(script.len() as u64 / 16);
        script.extend(&es);
        script.extend(vec![0.0; 12 * 16_000]);
        starts.push(script.len() as u64 / 16);
        script.extend(&en);
        script.extend(vec![0.0; 4 * 16_000]);
        println!("frases en {starts:?} ms; total {} ms", script.len() / 16);

        let recording = Arc::new(AtomicBool::new(true));
        let writer = {
            let (path, recording) = (dir.join("audio/system.wav"), recording.clone());
            std::thread::spawn(move || {
                let mut w = WavWriter::create(&path, 16_000).unwrap();
                for block in script.chunks(16_000) {
                    w.write(block).unwrap(); // 1 s de audio cada 50 ms
                    std::thread::sleep(Duration::from_millis(50));
                }
                w.finish().unwrap();
                recording.store(false, Ordering::Release);
            })
        };
        let model = std::env::var("SCREENCUT_MODEL").unwrap_or("ggml-small-q5_1.bin".into());
        let job = Job {
            id: "probe".into(),
            dir: dir.clone(),
            language: "auto".into(),
            model_id: "probe",
            model: models.join(model),
            vad: Some(models.join("ggml-silero-v6.2.0.bin")),
            recording,
        };
        let mut all = Vec::new();
        run(&job, |segs| { all.extend(segs); Ok(()) }, |d, t| println!("progreso {d}/{t}")).unwrap();
        writer.join().unwrap();
        crate::infra::stt::unload();
        for s in transcript::clean(all.clone()) {
            println!("{:>6}-{:>6} {} [{:?}] {}", s.start_ms, s.end_ms, s.speaker, s.lang, s.text);
        }
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(all.len(), 3, "una por frase");
        for (seg, start) in all.iter().zip(starts) {
            assert!(seg.start_ms.abs_diff(start) < 1_500, "{} empieza en {} y no en {start}", seg.text, seg.start_ms);
        }
    }
}
