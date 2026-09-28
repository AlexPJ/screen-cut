// Selector de objetivo: una ventana o un monitor (o, para una sesión, una
// región). Qué se hace con lo elegido lo decide el backend según el propósito
// con el que se abrió: "capture" o "session".
const { invoke } = window.__TAURI__.core;
const appWindow = window.__TAURI__.window.getCurrentWindow();
document.documentElement.dataset.theme = localStorage.getItem("theme") ||
  (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");

const grid = document.getElementById("grid");
let sources = { screens: [], windows: [], windows_error: null };
let tab = "windows";
let generation = 0; // descarta miniaturas de una lista anterior

function card(title, sub, target) {
  const el = document.createElement("button");
  el.className = "card";
  el.innerHTML = `<div class="thumb"></div><div class="card-title"></div><div class="card-sub"></div>`;
  el.querySelector(".card-title").textContent = title;
  el.querySelector(".card-sub").textContent = sub;
  el.title = title;
  el.dataset.target = JSON.stringify(target);
  el.onclick = () => invoke("choose_target", { target }).catch(showError);
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

invoke("get_picker_purpose").then((purpose) => {
  if (purpose !== "session") return;
  document.getElementById("hint").hidden = false;
  const region = document.getElementById("btn-region");
  region.hidden = false;
  region.onclick = () => {
    localStorage.setItem("overlay-mode", "target-session");
    invoke("choose_region_target").catch(showError);
  };
});

load();
