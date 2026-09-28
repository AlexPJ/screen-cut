// Línea de tiempo de una sesión: en qué intervalo vale cada imagen y cuál
// corresponde a un momento dado. Lo usan el visor de la app y el index.html
// exportado (y los tests con Node).
(function (root) {
  // Intervalo [start, end) de cada imagen, en ms desde el inicio de la sesión.
  // Por defecto: desde que se tomó hasta la siguiente (o el final), recortado a
  // `maxImageSecs` si hay duración máxima. `start_ms`/`end_ms` editados ganan.
  function ranges(images, durationMs, maxImageSecs) {
    return images.map((im, i) => {
      const next = images[i + 1];
      let end = next ? next.t_ms : Math.max(durationMs, im.t_ms);
      if (maxImageSecs) end = Math.min(end, im.t_ms + maxImageSecs * 1000);
      const start = im.start_ms ?? im.t_ms;
      return { start, end: Math.max(im.end_ms ?? end, start) };
    });
  }

  // Índice de la imagen vigente en `t` (-1 si ninguna). Si varias lo cubren
  // (rangos editados que se solapan), gana la que empezó más tarde.
  function imageAt(t, rs) {
    let best = -1;
    rs.forEach((r, i) => {
      if (t >= r.start && t < r.end && (best < 0 || r.start >= rs[best].start)) best = i;
    });
    return best;
  }

  const pad = (n) => String(n).padStart(2, "0");

  // 734000 → "12:14"; con `long` o a partir de una hora → "00:12:14".
  function fmt(ms, long) {
    const s = Math.floor(Math.max(0, ms) / 1000);
    const h = Math.floor(s / 3600), m = Math.floor(s / 60) % 60;
    return h || long ? `${pad(h)}:${pad(m)}:${pad(s % 60)}` : `${pad(m)}:${pad(s % 60)}`;
  }

  // "12:14", "1:02:03" o "45" → ms; null si no es válido.
  function parse(text) {
    const parts = String(text).trim().split(":");
    if (!parts.length || parts.length > 3 || parts.some((p) => !/^\d+$/.test(p))) return null;
    return parts.reduce((acc, p) => acc * 60 + Number(p), 0) * 1000;
  }

  const Timeline = { ranges, imageAt, fmt, parse };
  if (typeof module !== "undefined" && module.exports) module.exports = Timeline;
  else root.Timeline = Timeline;
})(this);
