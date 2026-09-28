// Selector de objetivo: una ventana o un monitor (o, para una sesión, una
// región). Qué se hace con lo elegido lo decide el backend según el propósito
// con el que se abrió: "capture", "session" o "record".
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const appWindow = window.__TAURI__.window.getCurrentWindow();
document.documentElement.dataset.theme = localStorage.getItem("theme") ||
  (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");

const grid = document.getElementById("grid");
let sources = { screens: [], windows: [], windows_error: null };
let tab = "windows";
let generation = 0; // descarta miniaturas de una lista anterior
let purpose = "capture";

// En una sesión, antes de elegir se guarda el idioma de la transcripción.
async function choose(action) {
  try {
    if (purpose === "session") {
      await invoke("set_session_language", {
        language: document.getElementById("lang").value,
        remember: document.getElementById("remember").checked,
      });
    }
    await action();
  } catch (e) {
    showError(e);
  }
}

function card(title, sub, target) {
  const el = document.createElement("button");
  el.className = "card";
  el.innerHTML = `<div class="thumb"></div><div class="card-title"></div><div class="card-sub"></div>`;
  el.querySelector(".card-title").textContent = title;
  el.querySelector(".card-sub").textContent = sub;
  el.title = title;
  el.dataset.target = JSON.stringify(target);
  el.onclick = () => choose(() => invoke("choose_target", { target }));
  return el;
}

function showError(e) {
  grid.innerHTML = "";
  const p = document.createElement("p");
  p.className = "empty";
  p.textContent = String(e);
  grid.appendChild(p);
}

function render() {
  const gen = ++generation;
  grid.innerHTML = "";
  const items = [];
  if (tab === "windows") {
    for (const w of sources.windows) {
      const title = w.title || w.app;
      const sub = (w.title ? w.app + " · " : "") + `${w.width} × ${w.height}`;
      items.push(card(title, sub, { kind: "window", id: w.id, title: w.title, app: w.app }));
    }
    if (!items.length) {
      showError(sources.windows_error || "No hay ventanas que se puedan capturar.");
      return;
    }
  } else {
    sources.screens.forEach((s, i) => {
      const title = `Pantalla ${i + 1}` + (s.primary ? " (principal)" : "");
      const px = `${Math.round(s.screen.width * s.screen.scale)} × ${Math.round(s.screen.height * s.screen.scale)}`;
      items.push(card(title, (s.name ? s.name + " · " : "") + px, { kind: "screen", screen: s.screen }));
    });
  }
  items.forEach((el) => grid.appendChild(el));
  loadThumbnails(items, gen);
}

// Miniaturas de tres en tres: capturar todas a la vez satura la CPU un momento.
async function loadThumbnails(items, gen) {
  const queue = [...items];
  const worker = async () => {
    while (queue.length && gen === generation) {
      const el = queue.shift();
      const thumb = el.querySelector(".thumb");
      const target = JSON.parse(el.dataset.target);
      try {
        const png = await invoke("source_thumbnail", { target });
        if (gen !== generation) return;
        const img = new Image();
        img.src = "data:image/png;base64," + png;
        thumb.appendChild(img);
      } catch (e) {
        thumb.innerHTML = `<span class="err"></span>`;
        thumb.firstChild.textContent = String(e);
      }
    }
  };
  await Promise.all([worker(), worker(), worker()]);
}

async function load() {
  grid.innerHTML = `<p class="empty">Buscando ventanas…</p>`;
  try {
    sources = await invoke("list_capture_sources");
    render();
  } catch (e) {
    showError(e);
  }
}

document.querySelectorAll(".tab").forEach((b) => {
  b.onclick = () => {
    tab = b.dataset.tab;
    document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("active", t === b));
    render();
  };
});
document.getElementById("btn-refresh").onclick = load;
document.getElementById("btn-cancel").onclick = () => appWindow.close();
addEventListener("keydown", (e) => { if (e.key === "Escape") appWindow.close(); });

const HINTS = {
  session: "Elige la ventana, la pantalla o la región que quieres fijar: durante la sesión, el atajo de captura la capturará al instante.",
  record: "Elige la ventana, la pantalla o la región que quieres grabar. Las ventanas de ScreenCut no salen en el vídeo.",
};

invoke("get_picker_purpose").then((p) => {
  purpose = p;
  if (!HINTS[purpose]) return;
  const hint = document.getElementById("hint");
  hint.textContent = HINTS[purpose];
  hint.hidden = false;
  const region = document.getElementById("btn-region");
  region.hidden = false;
  region.onclick = () => choose(() => {
    localStorage.setItem("overlay-mode", purpose === "session" ? "target-session" : "target-record");
    return invoke("choose_region_target");
  });
  if (purpose === "session") setupSession().catch(() => {});
  if (purpose === "record") setupRecording().catch(() => {});
});

// Qué audio lleva el vídeo. Se guarda al cambiarlo, así que la próxima vez
// sale igual.
async function setupRecording() {
  const s = await invoke("get_session_settings");
  const system = document.getElementById("rec-system");
  const mic = document.getElementById("rec-mic");
  system.checked = s.rec_system_audio;
  mic.checked = s.rec_mic;
  const save = (patch) => invoke("update_session_settings", { patch }).catch(showError);
  system.onchange = () => save({ rec_system_audio: system.checked });
  mic.onchange = () => save({ rec_mic: mic.checked });
  document.getElementById("record-opts").hidden = false;
}

const mb = (bytes) => Math.round(bytes / 1e6) + " MB";

// Idioma de la transcripción y aviso si no hay modelo descargado.
async function setupSession() {
  const [setup, info] = await Promise.all([invoke("session_setup"), invoke("get_transcription_info")]);
  const opts = document.getElementById("session-opts");
  opts.hidden = false;
  Languages.fill(document.getElementById("lang"), info.languages, setup.language);
  document.getElementById("remember").checked = setup.remember;

  const note = document.getElementById("model-note");
  const text = document.getElementById("model-text");
  const button = document.getElementById("btn-download");
  const bar = document.getElementById("model-progress");
  if (!setup.records_audio) {
    note.hidden = false;
    text.textContent = "Esta sesión no grabará audio: actívalo en Ajustes → Transcripción.";
    return;
  }
  if (setup.model) return;
  note.hidden = false;
  text.textContent = "No hay ningún modelo de transcripción descargado. Puedes empezar igualmente: " +
    "el audio se guarda y se transcribirá al terminar si el modelo ya está listo.";
  button.hidden = false;
  button.textContent = `Descargar «${setup.wanted.label}» (${mb(setup.wanted.size)})`;
  button.onclick = async () => {
    button.disabled = true;
    bar.hidden = false;
    try {
      await invoke("download_model", { id: setup.wanted.id });
    } catch (e) {
      text.textContent = "No se pudo descargar el modelo: " + e;
      button.disabled = false;
      bar.hidden = true;
    }
  };
  listen("model-download", (e) => {
    const p = e.payload;
    if (p.id !== setup.wanted.id) return;
    if (p.state === "progress") {
      bar.hidden = false;
      button.disabled = true;
      bar.value = p.done / p.total;
      button.textContent = `Descargando… ${mb(p.done)} de ${mb(p.total)}`;
    } else if (p.state === "done") {
      text.textContent = `Modelo «${setup.wanted.label}» listo: la sesión se transcribirá.`;
      button.hidden = bar.hidden = true;
    }
  });
}

load();
