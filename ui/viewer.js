// Visor de una sesión: imagen a la izquierda, transcripción a la derecha.
// Dentro de la app lee y guarda por comandos de Tauri; en el index.html
// exportado lee los datos incrustados y es de solo lectura.
(function () {
  const T = window.__TAURI__;
  const inApp = !!(T && T.core);
  const invoke = inApp ? T.core.invoke : null;
  const $ = (id) => document.getElementById(id);
  const SPEAKERS = { me: "Tú", others: "Otros" };

  let S = null;        // session.json
  let rs = [];         // intervalos de cada imagen
  let current = -1;    // imagen mostrada
  let cache = new Map();
  let progress = null; // { done_ms, total_ms } de la transcripción en curso

  function store(key, value) {
    try {
      if (value === undefined) return localStorage.getItem(key);
      localStorage.setItem(key, value);
    } catch { return null; }
  }

  if (inApp) {
    const theme = store("theme");
    if (theme) document.documentElement.dataset.theme = theme;
  }

  // Ruta relativa de la sesión → URL que puede mostrar un <img>.
  async function src(rel) {
    if (!inApp) return rel;
    if (!cache.has(rel)) {
      cache.set(rel, invoke("session_file", { id: S.id, path: rel }).then((b64) => "data:image/png;base64," + b64));
    }
    return cache.get(rel);
  }

  function showError(e) {
    $("v-title").textContent = "No se pudo abrir la sesión";
    $("v-meta").textContent = String(e);
  }

  async function open(id) {
    S = inApp ? await invoke("load_session", { id }) : JSON.parse($("session-data").textContent);
    cache = new Map();
    progress = null;
    render();
  }
  window.openSession = (id) => open(id).catch(showError);

  function render() {
    const date = new Date(S.started_at_ms);
    $("v-title").textContent = "Sesión del " + date.toLocaleString("es", { dateStyle: "long", timeStyle: "short" });
    document.title = $("v-title").textContent;
    const parts = [
      "Duración " + Timeline.fmt(S.duration_ms),
      S.images.length + (S.images.length === 1 ? " captura" : " capturas"),
      targetLabel(S.target),
    ];
    const audio = (S.audio || []).map((a) => SPEAKERS[a.speaker] || a.speaker);
    if (audio.length) parts.push("audio: " + audio.join(" y "));
    if (S.status === "interrupted") parts.push("interrumpida");
    $("v-meta").textContent = parts.join(" · ");
    $("v-max").value = S.max_image_secs ? String(S.max_image_secs) : "";
    $("v-reveal").hidden = !inApp;
    $("v-readonly").hidden = inApp;
    for (const el of [$("v-max"), $("v-start"), $("v-end"), $("v-reset")]) el.disabled = !inApp;

    renderStrip();
    renderTranscript();
    renderTranscriptState();
    recompute();
    select(S.images.length ? 0 : -1);
  }

  function targetLabel(t) {
    if (t.kind === "window") return "Ventana: " + (t.title || t.app);
    if (t.kind === "region") return `Región de ${t.width} × ${t.height}`;
    return "Pantalla completa";
  }

  function recompute() {
    rs = Timeline.ranges(S.images, S.duration_ms, S.max_image_secs);
    renderTimeline();
    if (current >= 0) select(current);
  }

  function renderStrip() {
    const strip = $("v-strip");
    strip.innerHTML = "";
    S.images.forEach((im, i) => {
      const b = document.createElement("button");
      b.className = "v-thumb";
      b.innerHTML = "<img alt='' /><span></span>";
      b.querySelector("span").textContent = `${im.n} · ${Timeline.fmt(im.t_ms)}`;
      src(im.thumb).then((u) => (b.querySelector("img").src = u)).catch(() => {});
      b.onclick = () => select(i);
      strip.appendChild(b);
    });
  }

  function renderTimeline() {
    const bar = $("v-timeline");
    bar.innerHTML = "";
    const total = Math.max(S.duration_ms, 1);
    rs.forEach((r, i) => {
      const block = document.createElement("div");
      block.className = "v-block" + (i === current ? " active" : "");
      block.style.left = (r.start / total) * 100 + "%";
      block.style.width = Math.max(((r.end - r.start) / total) * 100, 0.4) + "%";
      block.title = `Captura ${S.images[i].n}: ${Timeline.fmt(r.start)} – ${Timeline.fmt(r.end)}`;
      bar.appendChild(block);
    });
  }

  $("v-timeline").onclick = (e) => {
    const box = e.currentTarget.getBoundingClientRect();
    const t = ((e.clientX - box.left) / box.width) * S.duration_ms;
    showAt(t);
  };

  function renderTranscript() {
    const list = $("v-transcript");
    list.innerHTML = "";
    if (!S.segments.length) {
      const p = document.createElement("p");
      p.className = "v-empty";
      const t = S.transcript;
      p.textContent = !t
        ? "Esta sesión no grabó audio."
        : t.status === "running" || t.status === "pending"
          ? "La transcripción irá apareciendo aquí."
          : t.status === "no_model"
            ? "El audio está guardado, pero no había ningún modelo de transcripción. Descarga uno en Ajustes → Sesiones y transcripción y pulsa Transcribir."
            : t.status === "failed"
              ? "No se pudo transcribir el audio."
              : "No se reconoció ninguna frase en el audio.";
      list.appendChild(p);
      return;
    }
    for (const seg of S.segments) {
      const b = document.createElement("button");
      b.className = "v-line " + seg.speaker;
      b.innerHTML = "<time></time><b></b><span></span>";
      b.querySelector("time").textContent = Timeline.fmt(seg.start_ms);
      b.querySelector("b").textContent = (SPEAKERS[seg.speaker] || seg.speaker) + ":";
      b.querySelector("span").textContent = seg.text;
      b.onclick = () => showAt(seg.start_ms);
      b.dataset.start = seg.start_ms;
      list.appendChild(b);
    }
  }

  // Estado de la transcripción y botón para (re)transcribir.
  function renderTranscriptState() {
    const t = S.transcript;
    const el = $("v-tstatus");
    el.classList.remove("err");
    el.title = "";
    let text = "";
    if (t) {
      const lang = t.language === "auto" ? "idioma automático" : Languages.name(t.language);
      if (t.status === "running") {
        text = progress && progress.total_ms
          ? `Transcribiendo… ${Timeline.fmt(progress.done_ms)} de ${Timeline.fmt(progress.total_ms)}`
          : "Transcribiendo…";
      } else if (t.status === "pending") text = "Pendiente";
      else if (t.status === "no_model") text = "Sin modelo descargado";
      else if (t.status === "failed") {
        text = "Error";
        el.classList.add("err");
        el.title = t.error || "";
        if (t.error) text += ": " + t.error;
      } else text = lang;
    }
    el.textContent = text;

    const hasAudio = (S.audio || []).length > 0 || (t && S.status === "interrupted");
    const canRun = inApp && t && hasAudio && t.status !== "running";
    $("v-tactions").hidden = !canRun;
    if (canRun) {
      $("v-transcribe").textContent = S.segments.length ? "Transcribir de nuevo" : "Transcribir";
      $("v-transcribe").disabled = false;
      $("v-lang").value = t.language;
    }
  }

  // Muestra la imagen vigente en el momento `t` (o un aviso si no hay).
  function showAt(t) {
    const i = Timeline.imageAt(t, rs);
    if (i >= 0) return select(i);
    current = -1;
    $("v-img").hidden = true;
    $("v-empty").hidden = false;
    $("v-empty").textContent = `No hay ninguna imagen asignada al ${Timeline.fmt(t)}.`;
    $("v-cap").textContent = "";
    updateHighlights();
  }

  async function select(i) {
    current = i;
    const img = $("v-img");
    if (i < 0) {
      img.hidden = true;
      $("v-empty").hidden = false;
      $("v-empty").textContent = "Esta sesión no tiene capturas. Durante una sesión, pulsa el atajo de captura para añadir una.";
      $("v-cap").textContent = "";
      $("v-start").value = $("v-end").value = "";
      updateHighlights();
      return;
    }
    const im = S.images[i];
    $("v-cap").textContent = `Captura ${i + 1} de ${S.images.length} · tomada en ${Timeline.fmt(im.t_ms)}`;
    $("v-start").value = Timeline.fmt(rs[i].start, true);
    $("v-end").value = Timeline.fmt(rs[i].end, true);
    $("v-start").classList.remove("invalid");
    $("v-end").classList.remove("invalid");
    $("v-prev").disabled = i === 0;
    $("v-next").disabled = i === S.images.length - 1;
    updateHighlights();
    try {
      const url = await src(im.file);
      if (current !== i) return; // el usuario ya pasó a otra
      img.src = url;
      img.hidden = false;
      $("v-empty").hidden = true;
    } catch (e) {
      img.hidden = true;
      $("v-empty").hidden = false;
      $("v-empty").textContent = String(e);
    }
  }

  // `scroll` = false al recargar la transcripción, para no mover lo que se lee.
  function updateHighlights(scroll = true) {
    document.querySelectorAll(".v-thumb").forEach((b, i) => b.classList.toggle("active", i === current));
    document.querySelectorAll(".v-block").forEach((b, i) => b.classList.toggle("active", i === current));
    const r = rs[current];
    let first = null;
    document.querySelectorAll(".v-line").forEach((line) => {
      const t = Number(line.dataset.start);
      const inside = !!r && t >= r.start && t < r.end;
      line.classList.toggle("in-image", inside);
      if (inside && !first) first = line;
    });
    if (!scroll) return;
    document.querySelector(".v-thumb.active")?.scrollIntoView({ block: "nearest", inline: "nearest" });
    first?.scrollIntoView({ block: "nearest" });
  }

  $("v-prev").onclick = () => current > 0 && select(current - 1);
  $("v-next").onclick = () => current < S.images.length - 1 && select(current + 1);
  addEventListener("keydown", (e) => {
    if (e.target.closest("input, select")) return;
    if (e.key === "ArrowLeft") $("v-prev").click();
    if (e.key === "ArrowRight") $("v-next").click();
  });

  // ---------- Edición (solo dentro de la app) ----------
  function editRange(field, input) {
    if (current < 0) return;
    const ms = Timeline.parse(input.value);
    if (ms === null || ms > S.duration_ms) {
      input.classList.add("invalid");
      return;
    }
    S.images[current][field] = ms;
    recompute();
    save();
  }
  $("v-start").onchange = (e) => editRange("start_ms", e.target);
  $("v-end").onchange = (e) => editRange("end_ms", e.target);
  $("v-reset").onclick = () => {
    if (current < 0) return;
    S.images[current].start_ms = null;
    S.images[current].end_ms = null;
    recompute();
    save();
  };
  $("v-max").onchange = (e) => {
    S.max_image_secs = e.target.value ? Number(e.target.value) : null;
    recompute();
    save();
  };
  $("v-reveal").onclick = () => invoke("reveal_session", { id: S.id }).catch(showError);

  // ---------- Transcripción (solo dentro de la app) ----------
  if (inApp) {
    invoke("get_transcription_info")
      .then((info) => {
        Languages.fill($("v-lang"), info.languages, S && S.transcript ? S.transcript.language : "auto");
      })
      .catch(() => {});

    $("v-transcribe").onclick = async () => {
      if (S.segments.length) {
        const ok = await T.dialog.ask("Se sustituirá la transcripción actual.", { title: "Transcribir de nuevo", kind: "warning" });
        if (!ok) return;
      }
      $("v-transcribe").disabled = true;
      try {
        await invoke("transcribe_session", { id: S.id, language: $("v-lang").value });
        await refreshTranscript();
      } catch (e) {
        $("v-transcribe").disabled = false;
        $("v-tstatus").textContent = String(e);
        $("v-tstatus").classList.add("err");
      }
    };

    // Recarga solo la transcripción, sin tocar la imagen que se está viendo.
    async function refreshTranscript() {
      const fresh = await invoke("load_session", { id: S.id });
      S.segments = fresh.segments;
      S.transcript = fresh.transcript;
      S.audio = fresh.audio;
      renderTranscript();
      renderTranscriptState();
      updateHighlights(false);
    }
    let reloadTimer = null;
    T.event.listen("session-transcript", (e) => {
      const p = e.payload;
      if (!S || p.id !== S.id) return;
      progress = p.status === "running" ? p : null;
      if (p.status !== "running") {
        clearTimeout(reloadTimer);
        reloadTimer = null;
        refreshTranscript().catch(showError);
        return;
      }
      renderTranscriptState();
      if (!reloadTimer) {
        reloadTimer = setTimeout(() => {
          reloadTimer = null;
          refreshTranscript().catch(() => {});
        }, 1000);
      }
    });
  }

  let saveTimer;
  function save() {
    if (!inApp) return;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      const edits = S.images.map((im) => ({ n: im.n, start_ms: im.start_ms ?? null, end_ms: im.end_ms ?? null }));
      invoke("save_session_edits", { id: S.id, edits, maxImageSecs: S.max_image_secs ?? null }).catch(showError);
    }, 400);
  }

  // ---------- Barra lateral redimensionable ----------
  const savedWidth = store("viewer-side-w");
  if (savedWidth) document.documentElement.style.setProperty("--side-w", savedWidth);
  $("v-resizer").addEventListener("pointerdown", (e) => {
    const resizer = e.currentTarget;
    resizer.setPointerCapture(e.pointerId);
    resizer.classList.add("dragging");
    const move = (ev) => {
      const w = Math.min(Math.max(window.innerWidth - ev.clientX, 240), window.innerWidth * 0.7);
      document.documentElement.style.setProperty("--side-w", w + "px");
    };
    const up = () => {
      resizer.classList.remove("dragging");
      resizer.removeEventListener("pointermove", move);
      store("viewer-side-w", getComputedStyle(document.documentElement).getPropertyValue("--side-w").trim());
    };
    resizer.addEventListener("pointermove", move);
    resizer.addEventListener("pointerup", up, { once: true });
  });

  window.openSession(inApp ? window.__SESSION_ID__ : null);
})();
