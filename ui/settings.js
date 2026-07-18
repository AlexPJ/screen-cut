// Ajustes + Acerca de: atajo Impr Pant, inicio con Windows, versión y updates.
(() => {
  const T = window.__TAURI__;
  const invoke = T.core.invoke;
  const $ = (id) => document.getElementById(id);

  const overlay = $("settings-overlay");
  const open = () => { overlay.classList.remove("hidden"); refreshAbout(); };
  const close = () => overlay.classList.add("hidden");

  $("btn-settings").onclick = open;
  $("settings-close").onclick = close;
  overlay.addEventListener("mousedown", (e) => { if (e.target === overlay) close(); });
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !overlay.classList.contains("hidden")) close();
  });

  function status(msg, kind = "") {
    const el = $("update-status");
    el.textContent = msg;
    el.className = "update-status " + kind;
  }

  // ---- Impr Pant ----
  const prtsc = $("opt-prtsc");
  prtsc.checked = localStorage.getItem("prtsc") === "1";
  prtsc.onchange = async () => {
    try {
      await invoke("set_prtsc_shortcut", { enabled: prtsc.checked });
      localStorage.setItem("prtsc", prtsc.checked ? "1" : "0");
    } catch (e) {
      prtsc.checked = !prtsc.checked;
      status("No se pudo cambiar el atajo: " + e, "err");
    }
  };
  // Aplica la preferencia guardada al arrancar.
  if (prtsc.checked) invoke("set_prtsc_shortcut", { enabled: true }).catch(() => {});

  // ---- Inicio con Windows ----
  const autostart = $("opt-autostart");
  T.autostart.isEnabled().then((v) => (autostart.checked = v)).catch(() => {});
  autostart.onchange = async () => {
    try {
      if (autostart.checked) await T.autostart.enable();
      else await T.autostart.disable();
    } catch (e) {
      autostart.checked = !autostart.checked;
      status("No se pudo cambiar el inicio automático: " + e, "err");
    }
  };

  // ---- Acerca de / versión ----
  async function refreshAbout() {
    try { $("about-version").textContent = "v" + (await T.app.getVersion()); } catch {}
  }

  // ---- Actualizaciones ----
  $("btn-update").onclick = async () => {
    const btn = $("btn-update");
    btn.disabled = true;
    status("Comprobando…");
    try {
      const update = await T.updater.check();
      if (!update) {
        status("Estás en la última versión ✓", "ok");
        return;
      }
      status(`Nueva versión ${update.version} disponible. Descargando…`);
      let total = 0, got = 0;
      await update.downloadAndInstall((ev) => {
        if (ev.event === "Started") total = ev.data.contentLength || 0;
        else if (ev.event === "Progress") {
          got += ev.data.chunkLength || 0;
          if (total) status(`Descargando… ${Math.round((got / total) * 100)}%`);
        } else if (ev.event === "Finished") status("Instalando y reiniciando…");
      });
      await T.process.relaunch();
    } catch (e) {
      const msg = String(e);
      // Endpoint aún sin publicar / sin red: mensaje amable.
      if (/404|network|error sending|failed to lookup|dns|could not fetch|release json|remote/i.test(msg)) {
        status("No hay actualizaciones publicadas todavía.", "");
      } else {
        status("Error al comprobar: " + msg, "err");
      }
    } finally {
      btn.disabled = false;
    }
  };
})();
