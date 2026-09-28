// Ajustes + Acerca de: atajo Impr Pant, inicio con el sistema, versión y updates.
(() => {
  const T = window.__TAURI__;
  const invoke = T.core.invoke;
  const $ = (id) => document.getElementById(id);
  const platform = document.documentElement.dataset.platform;

  // Textos según plataforma (el HTML trae los de Windows).
  if (platform === "mac") {
    $("row-prtsc").style.display = "none"; // los teclados de Mac no tienen Impr Pant
    $("autostart-label").textContent = "Abrir al iniciar sesión";
    $("tray-note").innerHTML = "Al cerrar la ventana, la app sigue en la barra de menús para responder al atajo. Para salir del todo: icono de la barra de menús → <b>Salir</b>.";
  } else if (platform === "linux") {
    $("prtsc-desc").textContent = "Pulsa Impr Pant para capturar una región. Si tu escritorio ya usa esa tecla para su propia herramienta de capturas, desactívala allí.";
    $("autostart-label").textContent = "Iniciar al iniciar sesión";
  }

  const overlay = $("settings-overlay");
  const open = () => { overlay.classList.remove("hidden"); refreshAbout(); refreshFolder(); refreshTranscription(); };
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

  // ---- Inicio con el sistema ----
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

  // ---- Carpeta de capturas (autoguardado) ----
  const folderInput = $("opt-folder");
  async function refreshFolder() {
    try { folderInput.value = await invoke("get_screenshots_dir"); } catch {}
  }
  $("btn-folder-browse").onclick = async () => {
    try {
      const dir = await T.dialog.open({ directory: true, defaultPath: folderInput.value || undefined });
      if (!dir) return;
      await invoke("set_screenshots_dir", { path: dir });
      folderInput.value = dir;
    } catch (e) {
      status("No se pudo cambiar la carpeta: " + e, "err");
    }
  };
  $("btn-folder-reset").onclick = async () => {
    try {
      const def = await invoke("get_default_screenshots_dir");
      await invoke("set_screenshots_dir", { path: def });
      folderInput.value = def;
    } catch (e) {
      status("No se pudo restaurar la carpeta: " + e, "err");
    }
  };

  // ---- Sesiones y transcripción ----
  const mb = (bytes) => Math.round(bytes / 1e6) + " MB";
  const progress = {}; // id → { done, total } de las descargas en curso
  const bars = {}; // id → <progress> mostrado
  function modelStatus(msg, kind = "") {
    const el = $("models-status");
    el.textContent = msg;
    el.className = "update-status " + kind;
  }

  async function refreshModels() {
    const models = await invoke("list_models");
    const box = $("models");
    box.innerHTML = "";
    for (const m of models) {
      const row = document.createElement("label");
      row.className = "model-row" + (m.selected ? " selected" : "");
      row.innerHTML = `<input type="radio" name="model" /><span class="model-info"><b></b><small></small></span><span class="model-state"></span>`;
      row.querySelector("input").checked = m.selected;
      row.querySelector("b").textContent = `${m.label} · ${mb(m.size)}`;
      if (m.recommended) {
        const badge = document.createElement("span");
        badge.className = "model-badge";
        badge.textContent = "Recomendado para este equipo";
        row.querySelector("b").append(" ", badge);
      }
      row.querySelector("small").textContent = m.description;
      const state = row.querySelector(".model-state");
      const action = document.createElement("button");
      action.className = "btn btn-small";
      if (m.downloading || progress[m.id]) {
        const p = progress[m.id] || { done: 0, total: m.size };
        const bar = document.createElement("progress");
        bar.max = 1;
        bar.value = p.done / p.total;
        bar.title = `${mb(p.done)} de ${mb(p.total)}`;
        state.appendChild(bar);
        bars[m.id] = bar;
        action.textContent = "Cancelar";
        action.onclick = (e) => { e.preventDefault(); invoke("cancel_model_download", { id: m.id }); };
      } else if (m.installed) {
        state.textContent = "Descargado";
        action.textContent = "Borrar";
        action.title = "Libera " + mb(m.size);
        action.onclick = async (e) => {
          e.preventDefault();
          try { await invoke("delete_model", { id: m.id }); } catch (err) { modelStatus(String(err), "err"); }
          refreshModels();
        };
      } else {
        action.textContent = "Descargar";
        action.onclick = (e) => {
          e.preventDefault();
          progress[m.id] = { done: 0, total: m.size };
          modelStatus("");
          invoke("download_model", { id: m.id }).catch((err) => modelStatus("No se pudo descargar: " + err, "err"));
          refreshModels();
        };
      }
      row.appendChild(action);
      row.querySelector("input").onchange = async () => {
        await invoke("set_whisper_model", { id: m.id }).catch((err) => modelStatus(String(err), "err"));
        refreshModels();
      };
      box.appendChild(row);
    }
  }

  T.event.listen("model-download", (e) => {
    const p = e.payload;
    if (p.state === "progress") {
      progress[p.id] = { done: p.done, total: p.total };
      const bar = bars[p.id];
      if (bar && bar.isConnected) {
        bar.value = p.done / p.total;
        bar.title = `${mb(p.done)} de ${mb(p.total)}`;
      } else if (!overlay.classList.contains("hidden")) {
        refreshModels();
      }
      return;
    }
    delete progress[p.id];
    if (p.state === "done") modelStatus("Modelo descargado ✓", "ok");
    if (p.state === "error") modelStatus("No se pudo descargar: " + p.error, "err");
    refreshModels();
  });

  $("btn-model-import").onclick = async () => {
    try {
      const path = await T.dialog.open({ filters: [{ name: "Modelo de Whisper (ggml)", extensions: ["bin"] }] });
      if (!path) return;
      modelStatus("Comprobando el archivo…");
      await invoke("import_model", { path });
      modelStatus("Modelo importado ✓", "ok");
      refreshModels();
    } catch (e) {
      modelStatus(String(e), "err");
    }
  };

  let sessionSettings = {};
  async function saveSessionSettings(patch) {
    try {
      sessionSettings = await invoke("update_session_settings", { patch });
    } catch (e) {
      modelStatus("No se pudo guardar: " + e, "err");
    }
    fillSessionSettings();
  }
  function fillSessionSettings() {
    const s = sessionSettings;
    $("opt-lang").value = s.remember_language ? s.transcription_language : "auto";
    $("opt-live").checked = s.transcribe_live;
    $("opt-mic").checked = s.session_mic;
    $("opt-system").checked = s.session_system_audio;
    $("opt-keep-audio").checked = s.keep_session_audio;
    $("opt-max").value = s.max_image_secs ? String(s.max_image_secs) : "";
    $("opt-sessions-folder").value = s.sessions_dir_effective || "";
  }
  let languagesLoaded = false;
  async function refreshTranscription() {
    try {
      if (!languagesLoaded) {
        const info = await invoke("get_transcription_info");
        Languages.fill($("opt-lang"), info.languages, "auto");
        languagesLoaded = true;
      }
      sessionSettings = await invoke("get_session_settings");
      fillSessionSettings();
      await refreshModels();
    } catch (e) {
      modelStatus(String(e), "err");
    }
  }
  $("opt-lang").onchange = (e) => {
    const lang = e.target.value;
    saveSessionSettings({ transcription_language: lang, remember_language: lang !== "auto" });
  };
  $("opt-live").onchange = (e) => saveSessionSettings({ transcribe_live: e.target.checked });
  $("opt-mic").onchange = (e) => saveSessionSettings({ session_mic: e.target.checked });
  $("opt-system").onchange = (e) => saveSessionSettings({ session_system_audio: e.target.checked });
  $("opt-keep-audio").onchange = (e) => saveSessionSettings({ keep_session_audio: e.target.checked });
  $("opt-max").onchange = (e) => saveSessionSettings({ max_image_secs: e.target.value ? Number(e.target.value) : null });
  $("btn-sessions-browse").onclick = async () => {
    const dir = await T.dialog.open({ directory: true, defaultPath: $("opt-sessions-folder").value || undefined }).catch(() => null);
    if (dir) saveSessionSettings({ sessions_dir: dir });
  };
  $("btn-sessions-reset").onclick = () => saveSessionSettings({ sessions_dir: null });
  // Por si algún sistema se queda sin grabación de vídeo.
  invoke("recording_status").then((s) => { $("set-recording").hidden = !s.supported; }).catch(() => {});
  if (platform === "linux") {
    $("rec-note").textContent = "Los vídeos (MP4, o WebM si faltan los codificadores de GStreamer) se guardan en la carpeta «Grabaciones», dentro de la de capturas. En Wayland, el sistema te pedirá elegir la pantalla o ventana al empezar.";
  }

  // ---- Acerca de / versión ----
  async function refreshAbout() {
    try { $("about-version").textContent = "v" + (await T.app.getVersion()); } catch {}
    try {
      const stt = await invoke("get_transcription_info");
      $("about-stt").textContent =
        `Transcripción local: ${stt.engine} ${stt.version} · ${stt.languages.length} idiomas`;
    } catch {}
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
