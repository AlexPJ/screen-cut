//! Carpetas de sesión en disco:
//!
//! ```text
//! <sessions_dir>/<id>/session.json
//!                    /images/001_00-02-13.png
//!                    /thumbs/001.png
//!                    /index.html
//! ```

use super::model::{Session, SessionSummary, Status, TranscriptStatus};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Serializa las escrituras de `session.json`: la sesión, el transcriptor y el
/// visor pueden querer guardar a la vez.
static WRITE: Mutex<()> = Mutex::new(());

pub const SESSION_FILE: &str = "session.json";

/// Carpeta de una sesión. El id llega del frontend, así que se valida para que
/// no pueda salirse de la carpeta de sesiones.
pub fn session_dir(root: &Path, id: &str) -> Result<PathBuf, String> {
    let valid = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err("Identificador de sesión no válido".into());
    }
    Ok(root.join(id))
}

pub fn read(dir: &Path) -> Result<Session, String> {
    let bytes = std::fs::read(dir.join(SESSION_FILE)).map_err(|e| format!("No se pudo abrir la sesión: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("session.json dañado: {e}"))
}

/// Escribe `session.json` de forma atómica (archivo temporal + renombrar), para
/// que un cierre inesperado nunca lo deje a medias.
pub fn write(dir: &Path, session: &Session) -> Result<(), String> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    write_atomic(&dir.join(SESSION_FILE), &serde_json::to_vec_pretty(session).map_err(|e| e.to_string())?)
}

/// Lee, modifica y guarda `session.json` sin que otra escritura se cuele en medio.
pub fn update<T>(dir: &Path, change: impl FnOnce(&mut Session) -> T) -> Result<(Session, T), String> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = read(dir)?;
    let out = change(&mut session);
    write_atomic(&dir.join(SESSION_FILE), &serde_json::to_vec_pretty(&session).map_err(|e| e.to_string())?)?;
    Ok((session, out))
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Todas las sesiones de `root`, la más reciente primero. Las carpetas sin
/// `session.json` válido se ignoran.
pub fn list(root: &Path) -> Vec<SessionSummary> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut list: Vec<SessionSummary> = entries
        .filter_map(|e| read(&e.ok()?.path()).ok())
        .map(|s| SessionSummary {
            target: s.target.label(),
            images: s.images.len(),
            transcript: s.transcript.as_ref().map(|t| t.status),
            id: s.id,
            started_at_ms: s.started_at_ms,
            duration_ms: s.duration_ms,
            status: s.status,
        })
        .collect();
    list.sort_by_key(|s| std::cmp::Reverse(s.started_at_ms));
    list
}

/// Al arrancar no hay ninguna sesión en curso ni transcribiéndose: las que
/// siguen marcadas así se quedaron a medias al cerrarse la app.
pub fn mark_interrupted(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for dir in entries.filter_map(|e| Some(e.ok()?.path())) {
        let Ok(s) = read(&dir) else { continue };
        let transcribing = s.transcript.as_ref().is_some_and(|t| t.status == TranscriptStatus::Running);
        if s.status == Status::Recording || transcribing {
            let _ = update(&dir, |s| {
                if s.status == Status::Recording {
                    s.status = Status::Interrupted;
                }
                if let Some(t) = s.transcript.as_mut().filter(|t| t.status == TranscriptStatus::Running) {
                    t.status = TranscriptStatus::Failed;
                    t.error = Some("La app se cerró antes de terminar la transcripción".into());
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ids_that_escape_the_folder() {
        let root = Path::new("/tmp/s");
        assert!(session_dir(root, "2026-09-28_10-15-03-120").is_ok());
        assert!(session_dir(root, "../etc").is_err());
        assert!(session_dir(root, "a/b").is_err());
        assert!(session_dir(root, "").is_err());
    }
}
