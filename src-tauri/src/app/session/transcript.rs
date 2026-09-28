//! Transcripción final de una sesión: ordenar los fragmentos de las dos pistas,
//! quitar el eco y exportarla a TXT, SRT y Markdown.

use super::model::{Segment, Session};

pub const SPEAKER_ME: &str = "me";
pub const SPEAKER_OTHERS: &str = "others";

pub fn speaker_name(speaker: &str) -> &str {
    match speaker {
        SPEAKER_ME => "Tú",
        SPEAKER_OTHERS => "Otros",
        other => other,
    }
}

/// Ordena por tiempo y quita el eco: sin auriculares, el micrófono recoge lo
/// que suena por los altavoces y "Tú" repetiría lo que dicen "Otros". Se
/// descarta un fragmento de "Tú" si al menos la mitad de su duración coincide
/// con fragmentos de "Otros" que dicen casi lo mismo.
pub fn clean(mut segments: Vec<Segment>) -> Vec<Segment> {
    segments.sort_by_key(|s| (s.start_ms, s.end_ms));
    let others: Vec<&Segment> = segments.iter().filter(|s| s.speaker == SPEAKER_OTHERS).collect();
    let echo: Vec<bool> = segments
        .iter()
        .map(|me| {
            if me.speaker != SPEAKER_ME {
                return false;
            }
            let overlapping: Vec<&&Segment> = others.iter().filter(|o| overlap(me, o) > 0).collect();
            let shared: u64 = overlapping.iter().map(|o| overlap(me, o)).sum();
            let text: Vec<&str> = overlapping.iter().map(|o| o.text.as_str()).collect();
            let long = me.end_ms.saturating_sub(me.start_ms).max(1);
            shared * 2 >= long && similarity(&me.text, &text.join(" ")) >= 0.6
        })
        .collect();
    segments.into_iter().zip(echo).filter(|(_, e)| !e).map(|(s, _)| s).collect()
}

fn overlap(a: &Segment, b: &Segment) -> u64 {
    a.end_ms.min(b.end_ms).saturating_sub(a.start_ms.max(b.start_ms))
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Parecido entre dos textos (0–1): qué parte de las palabras de `a` (el eco
/// candidato) aparecen en `b`.
fn similarity(a: &str, b: &str) -> f32 {
    let a = words(a);
    let mut b = words(b);
    if a.is_empty() {
        return 1.0;
    }
    let common = a
        .iter()
        .filter(|w| match b.iter().position(|x| x == *w) {
            Some(i) => {
                b.swap_remove(i);
                true
            }
            None => false,
        })
        .count();
    common as f32 / a.len() as f32
}

/// "01:02:03" (o "02:03" si dura menos de una hora).
fn clock(ms: u64, hours: bool) -> String {
    let s = ms / 1000;
    if hours {
        format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub fn to_txt(session: &Session) -> String {
    let hours = session.duration_ms >= 3_600_000;
    session
        .segments
        .iter()
        .map(|s| format!("[{}] {}: {}\n", clock(s.start_ms, hours), speaker_name(&s.speaker), s.text))
        .collect()
}

pub fn to_srt(session: &Session) -> String {
    let t = |ms: u64| format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    session
        .segments
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let end = s.end_ms.max(s.start_ms + 500);
            format!("{}\n{} --> {}\n{}: {}\n\n", i + 1, t(s.start_ms), t(end), speaker_name(&s.speaker), s.text)
        })
        .collect()
}

/// Markdown con las capturas intercaladas en su momento, para pegarlo en
/// notas o documentos. Las imágenes van por ruta relativa.
pub fn to_md(session: &Session) -> String {
    let hours = session.duration_ms >= 3_600_000;
    let mut out = format!("# Sesión de ScreenCut · {}\n\n", session.id);
    let mut images = session.images.iter().peekable();
    for seg in &session.segments {
        while let Some(img) = images.next_if(|i| i.t_ms <= seg.start_ms) {
            out += &format!("![Captura {} · {}]({})\n\n", img.n, clock(img.t_ms, hours), img.file);
        }
        out += &format!("**[{}] {}:** {}\n\n", clock(seg.start_ms, hours), speaker_name(&seg.speaker), seg.text);
    }
    for img in images {
        out += &format!("![Captura {} · {}]({})\n\n", img.n, clock(img.t_ms, hours), img.file);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::session::model::SessionImage;

    fn seg(speaker: &str, start: u64, end: u64, text: &str) -> Segment {
        Segment { start_ms: start, end_ms: end, speaker: speaker.into(), lang: None, text: text.into() }
    }

    #[test]
    fn removes_the_microphone_echo() {
        let out = clean(vec![
            seg("me", 1_200, 4_100, "¿Qué tal ha ido la semana?"),
            seg("others", 1_000, 4_000, "Qué tal ha ido la semana"),
            seg("me", 5_000, 7_000, "Bien, gracias."),
        ]);
        let texts: Vec<&str> = out.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["Qué tal ha ido la semana", "Bien, gracias."]);
    }

    #[test]
    fn keeps_real_overlapping_speech() {
        let out = clean(vec![
            seg("others", 1_000, 4_000, "Qué tal ha ido la semana"),
            seg("me", 2_000, 4_000, "Perdona, no te oigo"),
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].speaker, "others"); // ordenado por tiempo
    }

    #[test]
    fn exports_srt_and_markdown() {
        let mut s = crate::app::session::export::tests::sample();
        s.segments = vec![seg("others", 61_500, 63_000, "Hola"), seg("me", 70_000, 70_000, "Buenas")];
        s.images = vec![SessionImage {
            n: 1,
            file: "images/001_00-01-05.png".into(),
            thumb: "thumbs/001.png".into(),
            t_ms: 65_000,
            start_ms: None,
            end_ms: None,
            width: 10,
            height: 10,
        }];
        assert_eq!(
            to_srt(&s),
            "1\n00:01:01,500 --> 00:01:03,000\nOtros: Hola\n\n2\n00:01:10,000 --> 00:01:10,500\nTú: Buenas\n\n"
        );
        assert_eq!(to_txt(&s), "[01:01] Otros: Hola\n[01:10] Tú: Buenas\n");
        let md = to_md(&s);
        let (hola, img, buenas) = (md.find("Hola").unwrap(), md.find("![Captura 1").unwrap(), md.find("Buenas").unwrap());
        assert!(hola < img && img < buenas, "{md}");
    }
}
