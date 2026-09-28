//! `index.html` autocontenido de una sesión: el mismo visor que se usa dentro
//! de la app, con el CSS, el JS y los datos incrustados. Las imágenes van por
//! ruta relativa, así que funciona abriéndolo desde el Finder o el Explorador
//! mientras esté junto a sus carpetas `images/` y `thumbs/`.

use super::model::Session;
use super::store::write_atomic;
use std::path::Path;

const HTML: &str = include_str!("../../../../ui/viewer.html");
const CSS: &str = include_str!("../../../../ui/viewer.css");
const TIMELINE_JS: &str = include_str!("../../../../ui/timeline.js");
const VIEWER_JS: &str = include_str!("../../../../ui/viewer.js");

const CSS_TAG: &str = r#"<link rel="stylesheet" href="viewer.css" />"#;
const JS_TAGS: &str = "<script src=\"timeline.js\"></script>\n  <script src=\"viewer.js\"></script>";

/// Dentro de un `<script>`, "</" cerraría la etiqueta antes de tiempo.
fn inline_script(code: &str) -> String {
    code.replace("</", "<\\/")
}

pub fn render(session: &Session) -> Result<String, String> {
    if !HTML.contains(CSS_TAG) || !HTML.contains(JS_TAGS) {
        return Err("La plantilla del visor no tiene las etiquetas esperadas".into());
    }
    let data = serde_json::to_string(session).map_err(|e| e.to_string())?;
    let scripts = format!(
        "<script id=\"session-data\" type=\"application/json\">{}</script>\n  <script>\n{}\n</script>\n  <script>\n{}\n</script>",
        inline_script(&data),
        inline_script(TIMELINE_JS),
        inline_script(VIEWER_JS),
    );
    Ok(HTML
        .replace(CSS_TAG, &format!("<style>\n{CSS}\n</style>"))
        .replace(JS_TAGS, &scripts))
}

pub fn write_index(dir: &Path, session: &Session) -> Result<(), String> {
    write_atomic(&dir.join("index.html"), render(session)?.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::session::model::{Segment, Status, FORMAT_VERSION};
    use crate::app::target::CaptureTarget;

    pub(crate) fn sample() -> Session {
        Session {
            version: FORMAT_VERSION,
            id: "2026-09-28_10-15-03-120".into(),
            started_at_ms: 1_790_000_000_000,
            duration_ms: 125_000,
            status: Status::Complete,
            target: CaptureTarget::Window { id: 1, title: "Zoom".into(), app: "zoom.us".into() },
            max_image_secs: None,
            images: Vec::new(),
            segments: vec![Segment {
                start_ms: 0,
                end_ms: 1000,
                speaker: "others".into(),
                lang: None,
                text: "</script><script>alert(1)</script>".into(),
            }],
        }
    }

    #[test]
    fn renders_a_self_contained_page() {
        let html = render(&sample()).unwrap();
        assert!(!html.contains(CSS_TAG) && !html.contains("src=\"viewer.js\""));
        assert!(html.contains("id=\"session-data\""));
        // Solo el cierre de los <script> propios; el texto no puede cerrarlos.
        assert_eq!(html.matches("</script>").count(), 3);
    }
}

