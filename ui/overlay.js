// Overlay de selección de región sobre el escritorio congelado.
const { invoke } = window.__TAURI__.core;

const mode = localStorage.getItem("overlay-mode") || "region";
localStorage.setItem("overlay-mode", "region");

const screenImg = document.getElementById("screen");
const sel = document.getElementById("sel");
const dim = document.getElementById("dim");
const sizeTag = document.getElementById("size");
const help = document.getElementById("help");

if (mode === "scroll-down") {
  help.innerHTML = "Selecciona la zona con <b>scroll vertical</b> · Esc para cancelar";
} else if (mode === "scroll-right") {
  help.innerHTML = "Selecciona la zona con <b>scroll horizontal</b> · Esc para cancelar";
}

invoke("get_capture_png", { which: "overlay" })
  .then((info) => { screenImg.src = "data:image/png;base64," + info.png_base64; })
  .catch(() => invoke("cancel_region_selection"));

let start = null;

function physical(e) {
  // El overlay ocupa exactamente el escritorio virtual; convertimos CSS px → físicos.
  const s = window.devicePixelRatio || 1;
  return { x: Math.round(e.clientX * s), y: Math.round(e.clientY * s) };
}

addEventListener("dragstart", (e) => e.preventDefault());

addEventListener("mousedown", (e) => {
  if (e.button !== 0) return;
  e.preventDefault();
  start = { cx: e.clientX, cy: e.clientY, ...physical(e) };
  dim.classList.add("off");
  sel.style.display = "block";
});

addEventListener("mousemove", (e) => {
  if (!start) return;
  const x = Math.min(start.cx, e.clientX);
  const y = Math.min(start.cy, e.clientY);
  const w = Math.abs(e.clientX - start.cx);
  const h = Math.abs(e.clientY - start.cy);
  Object.assign(sel.style, { left: x + "px", top: y + "px", width: w + "px", height: h + "px" });
  const p = physical(e);
  sizeTag.style.display = "block";
  sizeTag.textContent = `${Math.abs(p.x - start.x)} × ${Math.abs(p.y - start.y)}`;
  sizeTag.style.left = (e.clientX + 14) + "px";
  sizeTag.style.top = (e.clientY + 14) + "px";
});

addEventListener("mouseup", (e) => {
  if (!start || e.button !== 0) return;
  const p = physical(e);
  const x = Math.min(start.x, p.x);
  const y = Math.min(start.y, p.y);
  const width = Math.abs(p.x - start.x);
  const height = Math.abs(p.y - start.y);
  start = null;
  if (width < 8 || height < 8) {
    dim.classList.remove("off");
    sel.style.display = "none";
    sizeTag.style.display = "none";
    return;
  }
  invoke("finish_region_selection", { x, y, width, height, mode });
});

addEventListener("keydown", (e) => {
  if (e.key === "Escape") invoke("cancel_region_selection");
});
