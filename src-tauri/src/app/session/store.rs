//! Carpetas de sesión en disco:
//!
//! ```text
//! <sessions_dir>/<id>/session.json
//!                    /images/001_00-02-13.png
//!                    /thumbs/001.png
//!                    /index.html
//! ```

use super::model::{Session, SessionSummary, Status};
use std::path::{Path, PathBuf};

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
    write_atomic(&dir.join(SESSION_FILE), &serde_json::to_vec_pretty(session).map_err(|e| e.to_string())?)
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
            id: s.id,
            started_at_ms: s.started_at_ms,
            duration_ms: s.duration_ms,
            status: s.status,
        })
        .collect();
    list.sort_by_key(|s| std::cmp::Reverse(s.started_at_ms));
    list
}

/// Al arrancar no hay ninguna sesión en curso: las que siguen marcadas como
/// "recording" se quedaron a medias al cerrarse la app.
pub fn mark_interrupted(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for dir in entries.filter_map(|e| Some(e.ok()?.path())) {
        if let Ok(mut s) = read(&dir) {
            if s.status == Status::Recording {
                s.status = Status::Interrupted;
                let _ = write(&dir, &s);
            }
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
