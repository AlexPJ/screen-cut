// ScreenCut — ventana principal
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const appWindow = window.__TAURI__.window.getCurrentWindow();

const $ = (id) => document.getElementById(id);

// ---------- Tema (persistido) ----------
const savedTheme = localStorage.getItem("theme") ||
  (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
document.documentElement.dataset.theme = savedTheme;
$("btn-theme").onclick = () => {
  const next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
  document.documentElement.dataset.theme = next;
  localStorage.setItem("theme", next);
};

// ---------- Barra de título propia ----------
$("tb-min").onclick = () => appWindow.minimize();
$("tb-max").onclick = () => appWindow.toggleMaximize();
$("tb-close").onclick = () => appWindow.close();

$("titlebar").addEventListener("mousedown", (e) => {
  if (e.button !== 0 || e.target.closest(".tb-win")) return;
  if (e.detail === 2) appWindow.toggleMaximize(); // doble clic
  else appWindow.startDragging();
});

async function syncMaximized() {
  document.body.classList.toggle("maximized", await appWindow.isMaximized());
}
window.addEventListener("resize", syncMaximized);
syncMaximized();

// ---------- Utilidades ----------
let toastTimer;
function toast(msg) {
  const el = $("toast");
  el.textContent = msg;
  el.classList.remove("hidden");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.add("hidden"), 2200);
}
async function safe(fn) {
  try { return await fn(); }
  catch (e) { toast(typeof e === "string" ? e : (e?.message || "Error inesperado")); }
}

// ---------- Temporizador ----------
const timerSel = $("timer-select");
timerSel.value = localStorage.getItem("timer") ?? "3";
timerSel.onchange = () => localStorage.setItem("timer", timerSel.value);

function countdown() {
  const secs = parseInt(timerSel.value, 10) || 0;
  if (secs <= 0) return Promise.resolve();
  return new Promise((resolve) => {
    const el = $("countdown");
    let n = secs;
    el.textContent = n;
    el.classList.remove("hidden");
    const tick = setInterval(() => {
      n -= 1;
      if (n <= 0) { clearInterval(tick); el.classList.add("hidden"); resolve(); }
      else el.textContent = n;
    }, 1000);
  });
}

// ---------- Captura ----------
function showCaptureUI(info) {
  $("empty-state").classList.add("hidden");
  $("editor-wrap").classList.remove("hidden");
  $("anno-bar").classList.remove("hidden");
  $("capture-bar").classList.remove("hidden");
  $("progress").classList.add("hidden");
  $("ocr-panel").classList.add("hidden");
  Editor.show();
  Editor.load(info.png_base64);
  $("capture-meta").textContent = `${info.width} × ${info.height} px`;
}

let capturing = false; // evita countdowns solapados al pulsar dos veces
async function startCapture(mode) {
  if (capturing) return;
  capturing = true;
  try {
    localStorage.setItem("overlay-mode", mode === "full" ? "region" : mode);
    await countdown();
    if (mode === "full") return await safe(() => invoke("capture_fullscreen"));
    return await safe(() => invoke("start_region_selection"));
  } finally {
    capturing = false;
  }
}

$("btn-full").onclick = () => startCapture("full");
$("btn-region").onclick = () => startCapture("region");
$("btn-scroll-v").onclick = () => startCapture("scroll-down");
$("btn-scroll-h").onclick = () => startCapture("scroll-right");

// ---------- Barra de anotación ----------
const tools = document.querySelectorAll("#tools .tool");
function selectTool(name) {
  Editor.setTool(name);
  tools.forEach((b) => b.classList.toggle("active", b.dataset.tool === name));
}
tools.forEach((b) => (b.onclick = () => selectTool(b.dataset.tool)));
selectTool("arrow");

// Paleta de colores
const PALETTE = ["#e5484d", "#d97757", "#ffc53d", "#46a758", "#3b82f6", "#8b5cf6", "#ffffff", "#111111"];
const swatches = $("swatches");
function setColor(c) {
  Editor.setStyle("color", c);
  $("color-custom").value = c;
  swatches.querySelectorAll(".swatch").forEach((s) => s.classList.toggle("active", s.dataset.c === c));
}
PALETTE.forEach((c) => {
  const s = document.createElement("button");
  s.className = "swatch";
  s.dataset.c = c;
  s.style.background = c;
  s.onclick = () => setColor(c);
  swatches.appendChild(s);
});
$("color-custom").oninput = (e) => { Editor.setStyle("color", e.target.value); swatches.querySelectorAll(".swatch").forEach((s) => s.classList.remove("active")); };
setColor("#d97757");

// Relleno
const fillBtns = document.querySelectorAll("#fill-group .opt");
function setFill(mode) {
  Editor.setStyle("fillMode", mode);
  fillBtns.forEach((b) => b.classList.toggle("active", b.dataset.fill === mode));
}
fillBtns.forEach((b) => (b.onclick = () => setFill(b.dataset.fill)));
setFill("none");
$("fill-custom").oninput = (e) => { Editor.setStyle("fillColor", e.target.value); setFill("other"); };

// Grosor
$("stroke-width").oninput = (e) => Editor.setStyle("width", parseInt(e.target.value, 10));

// Deshacer/rehacer/limpiar
$("btn-undo").onclick = () => Editor.undo();
$("btn-redo").onclick = () => Editor.redo();
$("btn-clear").onclick = () => Editor.clear();
window.addEventListener("keydown", (e) => {
  // No interferir al escribir en el texto de anotación u OCR
  if (e.target.closest("textarea, input, select")) return;
  if (e.ctrlKey && e.key.toLowerCase() === "z") { e.preventDefault(); Editor.undo(); }
  if (e.ctrlKey && (e.key.toLowerCase() === "y")) { e.preventDefault(); Editor.redo(); }
  if (e.key === "Delete" || e.key === "Backspace") Editor.deleteSelected();
});

// ---------- Acciones finales ----------
$("btn-ocr").onclick = () => safe(async () => {
  if (!Editor.hasImage()) return;
  $("btn-ocr").disabled = true;
  toast("Reconociendo texto…");
  try {
    const result = await invoke("ocr_png", { pngBase64: Editor.basePng() });
    $("ocr-panel").classList.remove("hidden");
    $("ocr-text").value = result.text || "(sin texto reconocido)";
    $("ocr-lang").textContent = result.language ? `· ${result.language}` : "";
  } finally { $("btn-ocr").disabled = false; }
});
$("btn-copy-text").onclick = () => safe(async () => {
  await navigator.clipboard.writeText($("ocr-text").value);
  toast("Texto copiado");
});
$("btn-copy").onclick = () => safe(async () => {
  if (!Editor.hasImage()) return;
  await invoke("copy_png", { pngBase64: Editor.flattenedPng() });
  toast("Imagen copiada al portapapeles");
});
$("btn-save").onclick = () => safe(async () => {
  if (!Editor.hasImage()) return;
  const path = await window.__TAURI__.dialog.save({
    filters: [{ name: "Imagen PNG", extensions: ["png"] }],
    defaultPath: `captura-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-")}.png`,
  });
  if (!path) return;
  await invoke("save_png", { path, pngBase64: Editor.flattenedPng() });
  toast("Guardado en " + path);
});

// ---------- Eventos del backend ----------
listen("capture-ready", (e) => showCaptureUI(e.payload));
listen("capture-error", (e) => { $("progress").classList.add("hidden"); toast(e.payload); });
listen("scroll-progress", (e) => {
  const p = $("progress");
  p.classList.remove("hidden");
  p.textContent = `Capturando con scroll… ${e.payload.total_px}px`;
});


