// Editor de anotaciones sobre la captura. Arquitectura de dos capas:
//  - base-canvas: los píxeles de la imagen (y el resultado tras un crop).
//  - anno-canvas: capa transparente donde se dibujan las formas; se redibuja
//    entera desde el modelo `shapes` en cada cambio (retenido, permite deshacer).
// El aplanado (base + anotaciones) se exporta a PNG para copiar/guardar/OCR.
// Historial por comandos (add/del/mod) para deshacer/rehacer coherentes.

const Editor = (() => {
  const wrap = document.getElementById("editor-wrap");
  const editor = document.getElementById("editor");
  const baseCanvas = document.getElementById("base-canvas");
  const annoCanvas = document.getElementById("anno-canvas");
  const baseCtx = baseCanvas.getContext("2d");
  const annoCtx = annoCanvas.getContext("2d");
  const cropActions = document.getElementById("crop-actions");

  let shapes = [];
  let history = []; // {kind:"add",shape} | {kind:"del",items:[{i,shape}]} | {kind:"mod",shape,before,after}
  let hIndex = 0;   // nº de entradas aplicadas
  let current = null;   // forma en curso
  let cropRect = null;  // {x,y,w,h} mientras se ajusta el recorte
  let dragging = false;
  let startPt = null;
  let textEl = null;
  let textCancelled = false;
  let moving = null;    // {shape, start, base, moved}
  let resizing = null;  // {shape, corner, bbox, base}
  let selected = null;  // forma seleccionada con la herramienta cursor
  let erasing = null;   // [{i, shape}] borradas en este arrastre del borrador

  const style = {
    tool: "arrow",
    color: "#d97757",
    fillMode: "none", // none | stroke | other
    fillColor: "#d97757",
    width: 4,
  };
  // Mientras sea false, el color de relleno sigue al color del trazo.
  let fillColorTouched = false;

  // ---------- utilidades ----------
  function hasImage() {
    return baseCanvas.width > 0 && baseCanvas.height > 0;
  }

  // ---------- zoom / ajuste a la ventana ----------
  const panel = wrap.parentElement; // .preview-panel: mide el espacio disponible
  let viewScale = 1;
  let fitMode = true;
  let viewCb = () => {};

  function fitScale() {
    if (!baseCanvas.width) return 1;
    const cs = getComputedStyle(panel);
    const padX = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight);
    const padY = parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom);
    const availW = panel.clientWidth - padX - 4;
    const availH = panel.clientHeight - padY - 4;
    return Math.max(0.05, Math.min(availW / baseCanvas.width, availH / baseCanvas.height, 1));
  }
  function effScale() { return fitMode ? fitScale() : viewScale; }
  function applyView() {
    if (!baseCanvas.width) return;
    const s = effScale();
    editor.style.width = Math.max(1, Math.round(baseCanvas.width * s)) + "px";
    viewCb(Math.round(s * 100), fitMode);
  }
  function setZoomPct(pct) {
    fitMode = false;
    viewScale = Math.max(0.1, Math.min(8, pct / 100));
    applyView();
  }
  function zoomIn() { setZoomPct(effScale() * 125); }
  function zoomOut() { setZoomPct(effScale() * 80); }
  function fitView() { fitMode = true; applyView(); }

  // Reaplica el ajuste cuando cambia el espacio disponible (ventana, panel OCR…).
  new ResizeObserver(() => { if (fitMode) applyView(); }).observe(panel);

  function setSize(w, h) {
    for (const c of [baseCanvas, annoCanvas]) {
      c.width = w;
      c.height = h;
    }
    editor.style.aspectRatio = `${w} / ${h}`;
    applyView();
  }

  function pointer(e) {
    const rect = annoCanvas.getBoundingClientRect();
    const sx = annoCanvas.width / rect.width;
    const sy = annoCanvas.height / rect.height;
    return {
      x: Math.max(0, Math.min(annoCanvas.width, (e.clientX - rect.left) * sx)),
      y: Math.max(0, Math.min(annoCanvas.height, (e.clientY - rect.top) * sy)),
    };
  }

  // canvas px por CSS px: tolerancias constantes en pantalla a cualquier escala
  function zoom() {
    const w = annoCanvas.getBoundingClientRect().width;
    return w > 0 ? annoCanvas.width / w : 1;
  }
  const tolPx = () => Math.max(6, 9 * zoom());

  function withAlpha(hex, a) {
    const n = parseInt(hex.slice(1), 16);
    return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
  }

  // ---------- snapshots (para mover/redimensionar y el historial) ----------
  function snapshot(s) {
    if (s.type === "pen") return { points: s.points.map((q) => ({ ...q })) };
    if (s.type === "text") return { x: s.x, y: s.y, size: s.size };
    return { x1: s.x1, y1: s.y1, x2: s.x2, y2: s.y2 };
  }
  function applySnap(s, snap) {
    if (s.type === "pen") s.points = snap.points.map((q) => ({ ...q }));
    else if (s.type === "text") { s.x = snap.x; s.y = snap.y; if (snap.size) s.size = snap.size; }
    else { s.x1 = snap.x1; s.y1 = snap.y1; s.x2 = snap.x2; s.y2 = snap.y2; }
  }

  function bbox(s) {
    if (s.type === "pen") {
      let x1 = Infinity, y1 = Infinity, x2 = -Infinity, y2 = -Infinity;
      for (const q of s.points) {
        x1 = Math.min(x1, q.x); y1 = Math.min(y1, q.y);
        x2 = Math.max(x2, q.x); y2 = Math.max(y2, q.y);
      }
      return { x1, y1, x2, y2 };
    }
    if (s.type === "text") {
      const lines = s.text.split("\n");
      const w = Math.max(...lines.map((l) => l.length)) * s.size * 0.6;
      return { x1: s.x, y1: s.y, x2: s.x + w, y2: s.y + lines.length * s.size * 1.25 };
    }
    return {
      x1: Math.min(s.x1, s.x2), y1: Math.min(s.y1, s.y2),
      x2: Math.max(s.x1, s.x2), y2: Math.max(s.y1, s.y2),
    };
  }
  const corners = (b) => [
    { x: b.x1, y: b.y1 }, { x: b.x2, y: b.y1 },
    { x: b.x1, y: b.y2 }, { x: b.x2, y: b.y2 },
  ];

  // ---------- historial ----------
  function histPush(entry) { history.length = hIndex; history.push(entry); hIndex++; }

  function undo() {
    if (!hIndex) return;
    const e = history[--hIndex];
    if (e.kind === "add") shapes.splice(shapes.indexOf(e.shape), 1);
    else if (e.kind === "del") for (const it of e.items) shapes.splice(Math.min(it.i, shapes.length), 0, it.shape);
    else applySnap(e.shape, e.before);
    if (selected && !shapes.includes(selected)) selected = null;
    renderAnno();
    notifyContext();
  }
  function redo() {
    if (hIndex >= history.length) return;
    const e = history[hIndex++];
    if (e.kind === "add") shapes.push(e.shape);
    else if (e.kind === "del") for (const it of [...e.items].reverse()) shapes.splice(shapes.indexOf(it.shape), 1);
    else applySnap(e.shape, e.after);
    if (selected && !shapes.includes(selected)) selected = null;
    renderAnno();
    notifyContext();
  }
  function clear() {
    if (!shapes.length) return;
    histPush({ kind: "del", items: shapes.map((s, i) => ({ i, shape: s })) });
    shapes = [];
    selected = null;
    renderAnno();
    notifyContext();
  }
  function deleteSelected() {
    if (!selected) return;
    const i = shapes.indexOf(selected);
    if (i >= 0) {
      shapes.splice(i, 1);
      histPush({ kind: "del", items: [{ i, shape: selected }] });
    }
    selected = null;
    renderAnno();
    notifyContext();
  }

  // ---------- render ----------
  function drawShape(ctx, s) {
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    ctx.lineWidth = s.width;
    ctx.strokeStyle = s.color;

    const fill = () => {
      if (s.fillMode === "stroke") ctx.fillStyle = withAlpha(s.color, 0.25);
      else if (s.fillMode === "other") ctx.fillStyle = s.fillColor;
      else return false;
      return true;
    };

    switch (s.type) {
      case "pen": {
        ctx.beginPath();
        s.points.forEach((p, i) => (i ? ctx.lineTo(p.x, p.y) : ctx.moveTo(p.x, p.y)));
        ctx.stroke();
        break;
      }
      case "line":
      case "arrow": {
        ctx.beginPath();
        ctx.moveTo(s.x1, s.y1);
        ctx.lineTo(s.x2, s.y2);
        ctx.stroke();
        if (s.type === "arrow") {
          const ang = Math.atan2(s.y2 - s.y1, s.x2 - s.x1);
          const len = Math.max(12, s.width * 3.5);
          for (const d of [-0.5, 0.5]) {
            ctx.beginPath();
            ctx.moveTo(s.x2, s.y2);
            ctx.lineTo(s.x2 - len * Math.cos(ang - d), s.y2 - len * Math.sin(ang - d));
            ctx.stroke();
          }
        }
        break;
      }
      case "rect": {
        const x = Math.min(s.x1, s.x2), y = Math.min(s.y1, s.y2);
        const w = Math.abs(s.x2 - s.x1), h = Math.abs(s.y2 - s.y1);
        if (fill()) ctx.fillRect(x, y, w, h);
        ctx.strokeRect(x, y, w, h);
        break;
      }
      case "ellipse": {
        const cx = (s.x1 + s.x2) / 2, cy = (s.y1 + s.y2) / 2;
        const rx = Math.abs(s.x2 - s.x1) / 2, ry = Math.abs(s.y2 - s.y1) / 2;
        ctx.beginPath();
        ctx.ellipse(cx, cy, rx, ry, 0, 0, Math.PI * 2);
        if (fill()) ctx.fill();
        ctx.stroke();
        break;
      }
      case "highlight": {
        const x = Math.min(s.x1, s.x2), y = Math.min(s.y1, s.y2);
        const w = Math.abs(s.x2 - s.x1), h = Math.abs(s.y2 - s.y1);
        ctx.fillStyle = withAlpha(s.color, 0.35);
        ctx.fillRect(x, y, w, h);
        break;
      }
      case "text": {
        ctx.font = `${s.size}px "Segoe UI", sans-serif`;
        ctx.textBaseline = "top";
        ctx.fillStyle = s.color;
        s.text.split("\n").forEach((ln, i) => ctx.fillText(ln, s.x, s.y + i * s.size * 1.25));
        break;
      }
    }
  }

  function renderAnno() {
    annoCtx.clearRect(0, 0, annoCanvas.width, annoCanvas.height);
    for (const s of shapes) drawShape(annoCtx, s);
    if (current) drawShape(annoCtx, current);
    if (cropRect) drawCrop();
    drawSelection();
  }

  function drawSelection() {
    if (!selected || style.tool !== "cursor" || !shapes.includes(selected)) return;
    const b = bbox(selected);
    const u = zoom();
    const hs = 8 * u;
    annoCtx.save();
    annoCtx.strokeStyle = "#3b82f6";
    annoCtx.lineWidth = Math.max(1, 1.5 * u);
    annoCtx.setLineDash([5 * u, 4 * u]);
    annoCtx.strokeRect(b.x1, b.y1, b.x2 - b.x1, b.y2 - b.y1);
    annoCtx.setLineDash([]);
    annoCtx.fillStyle = "#ffffff";
    for (const c of corners(b)) {
      annoCtx.fillRect(c.x - hs / 2, c.y - hs / 2, hs, hs);
      annoCtx.strokeRect(c.x - hs / 2, c.y - hs / 2, hs, hs);
    }
    annoCtx.restore();
  }

  function drawCrop() {
    const { x, y, w, h } = cropRect;
    const W = annoCanvas.width, H = annoCanvas.height;
    annoCtx.save();
    // Atenúa lo de fuera con 4 rectángulos: no toca lo de dentro (clearRect
    // borraría también las anotaciones ya dibujadas).
    annoCtx.fillStyle = "rgba(0,0,0,0.45)";
    annoCtx.fillRect(0, 0, W, y);
    annoCtx.fillRect(0, y + h, W, H - y - h);
    annoCtx.fillRect(0, y, x, h);
    annoCtx.fillRect(x + w, y, W - x - w, h);
    annoCtx.strokeStyle = "#d97757";
    annoCtx.lineWidth = 2;
    annoCtx.setLineDash([6, 4]);
    annoCtx.strokeRect(x, y, w, h);
    annoCtx.restore();
  }

  // ---------- interacción ----------
  function commit(shape) {
    shapes.push(shape);
    histPush({ kind: "add", shape });
    renderAnno();
  }

  // ---------- mover / redimensionar / borrar ----------
  function distToSeg(p, x1, y1, x2, y2) {
    const dx = x2 - x1, dy = y2 - y1;
    const len2 = dx * dx + dy * dy;
    const t = len2 ? Math.max(0, Math.min(1, ((p.x - x1) * dx + (p.y - y1) * dy) / len2)) : 0;
    return Math.hypot(p.x - (x1 + t * dx), p.y - (y1 + t * dy));
  }

  function hit(s, p) {
    const tol = tolPx() + (s.width || 4) / 2;
    switch (s.type) {
      case "pen":
        return s.points.some((q, i) => i && distToSeg(p, s.points[i - 1].x, s.points[i - 1].y, q.x, q.y) <= tol);
      case "line":
      case "arrow":
        return distToSeg(p, s.x1, s.y1, s.x2, s.y2) <= tol;
      case "rect":
      case "highlight": {
        const x = Math.min(s.x1, s.x2), y = Math.min(s.y1, s.y2);
        const w = Math.abs(s.x2 - s.x1), h = Math.abs(s.y2 - s.y1);
        const inside = p.x >= x - tol && p.x <= x + w + tol && p.y >= y - tol && p.y <= y + h + tol;
        if (s.type === "highlight" || s.fillMode !== "none") return inside;
        // sin relleno: solo cerca del borde
        const onEdge = Math.min(Math.abs(p.x - x), Math.abs(p.x - x - w), Math.abs(p.y - y), Math.abs(p.y - y - h)) <= tol;
        return inside && onEdge;
      }
      case "ellipse": {
        const cx = (s.x1 + s.x2) / 2, cy = (s.y1 + s.y2) / 2;
        const rx = Math.abs(s.x2 - s.x1) / 2 || 1, ry = Math.abs(s.y2 - s.y1) / 2 || 1;
        const d = ((p.x - cx) ** 2) / (rx * rx) + ((p.y - cy) ** 2) / (ry * ry);
        if (s.fillMode !== "none") return d <= 1 + tol / Math.min(rx, ry);
        return Math.abs(d - 1) <= tol / Math.min(rx, ry);
      }
      case "text": {
        const b = bbox(s);
        return p.x >= b.x1 - tol && p.x <= b.x2 + tol && p.y >= b.y1 - tol && p.y <= b.y2 + tol;
      }
    }
    return false;
  }

  function moveShape(s, base, dx, dy) {
    if (s.type === "pen") s.points = base.points.map((q) => ({ x: q.x + dx, y: q.y + dy }));
    else if (s.type === "text") { s.x = base.x + dx; s.y = base.y + dy; }
    else {
      s.x1 = base.x1 + dx; s.y1 = base.y1 + dy;
      s.x2 = base.x2 + dx; s.y2 = base.y2 + dy;
    }
  }

  function resizeShape(r, p) {
    const s = r.shape, b = r.bbox, base = r.base;
    if (s.type === "pen") {
      const ax = r.corner.x === b.x1 ? b.x2 : b.x1; // ancla = esquina opuesta
      const ay = r.corner.y === b.y1 ? b.y2 : b.y1;
      const sx = (p.x - ax) / (r.corner.x - ax || 1);
      const sy = (p.y - ay) / (r.corner.y - ay || 1);
      s.points = base.points.map((q) => ({ x: ax + (q.x - ax) * sx, y: ay + (q.y - ay) * sy }));
    } else if (s.type === "text") {
      const ax = r.corner.x === b.x1 ? b.x2 : b.x1;
      const ay = r.corner.y === b.y1 ? b.y2 : b.y1;
      const sx = Math.abs((p.x - ax) / (r.corner.x - ax || 1));
      const sy = Math.abs((p.y - ay) / (r.corner.y - ay || 1));
      s.size = Math.max(6, base.size * sy);
      s.x = ax + (base.x - ax) * sx;
      s.y = ay + (base.y - ay) * sy;
    } else if (s.type === "line" || s.type === "arrow") {
      // mueve el extremo más cercano a la esquina agarrada
      const d1 = Math.hypot(r.corner.x - base.x1, r.corner.y - base.y1);
      const d2 = Math.hypot(r.corner.x - base.x2, r.corner.y - base.y2);
      if (d1 <= d2) { s.x1 = p.x; s.y1 = p.y; } else { s.x2 = p.x; s.y2 = p.y; }
    } else {
      // rect/ellipse/highlight: arrastra la coordenada más cercana a la esquina
      if (Math.abs(r.corner.x - base.x1) <= Math.abs(r.corner.x - base.x2)) s.x1 = p.x; else s.x2 = p.x;
      if (Math.abs(r.corner.y - base.y1) <= Math.abs(r.corner.y - base.y2)) s.y1 = p.y; else s.y2 = p.y;
    }
  }

  function eraseAt(p) {
    let changed = false;
    for (let i = shapes.length - 1; i >= 0; i--) {
      if (hit(shapes[i], p)) {
        erasing.push({ i, shape: shapes[i] });
        shapes.splice(i, 1);
        changed = true;
      }
    }
    if (changed) {
      if (selected && !shapes.includes(selected)) selected = null;
      renderAnno();
    }
  }

  function onDown(e) {
    if (!hasImage() || e.button !== 0) return;
    const p = pointer(e);

    if (style.tool === "cursor") {
      // ¿un handle de redimensionado de la selección?
      if (selected) {
        const b = bbox(selected);
        for (const c of corners(b)) {
          if (Math.abs(p.x - c.x) <= tolPx() && Math.abs(p.y - c.y) <= tolPx()) {
            resizing = { shape: selected, corner: c, bbox: b, base: snapshot(selected) };
            dragging = true;
            return;
          }
        }
      }
      // ¿una forma bajo el cursor? → seleccionar y empezar a mover
      for (let i = shapes.length - 1; i >= 0; i--) {
        if (hit(shapes[i], p)) {
          selected = shapes[i];
          moving = { shape: shapes[i], start: p, base: snapshot(shapes[i]), moved: false };
          dragging = true;
          renderAnno();
          notifyContext();
          return;
        }
      }
      // vacío → deseleccionar
      if (selected) { selected = null; renderAnno(); }
      notifyContext();
      return;
    }

    if (style.tool === "eraser") {
      erasing = [];
      dragging = true;
      eraseAt(p);
      return;
    }

    if (style.tool === "text") {
      e.preventDefault(); // sin esto el mousedown le roba el foco al textarea al instante
      openTextInput(p, e);
      return;
    }

    dragging = true;
    startPt = p;
    if (style.tool === "crop") {
      cropRect = { x: p.x, y: p.y, w: 0, h: 0 };
      cropActions.classList.remove("hidden");
      return;
    }
    if (style.tool === "pen") {
      current = { type: "pen", points: [p], color: style.color, width: style.width };
    } else {
      current = {
        type: style.tool,
        x1: p.x, y1: p.y, x2: p.x, y2: p.y,
        color: style.color, width: style.width,
        fillMode: style.fillMode, fillColor: style.fillColor,
      };
    }
  }

  function onMove(e) {
    if (!dragging) return;
    const p = pointer(e);
    if (resizing) {
      resizeShape(resizing, p);
    } else if (moving) {
      moveShape(moving.shape, moving.base, p.x - moving.start.x, p.y - moving.start.y);
      moving.moved = true;
    } else if (erasing) {
      eraseAt(p);
      return; // eraseAt ya repinta si hace falta
    } else if (style.tool === "crop") {
      cropRect = {
        x: Math.min(startPt.x, p.x), y: Math.min(startPt.y, p.y),
        w: Math.abs(p.x - startPt.x), h: Math.abs(p.y - startPt.y),
      };
    } else if (current?.type === "pen") {
      current.points.push(p);
    } else if (current) {
      current.x2 = p.x;
      current.y2 = p.y;
    }
    renderAnno();
  }

  function onUp() {
    if (!dragging) return;
    dragging = false;
    if (moving) {
      if (moving.moved) {
        histPush({ kind: "mod", shape: moving.shape, before: moving.base, after: snapshot(moving.shape) });
      }
      moving = null;
      renderAnno();
      return;
    }
    if (resizing) {
      histPush({ kind: "mod", shape: resizing.shape, before: resizing.base, after: snapshot(resizing.shape) });
      resizing = null;
      renderAnno();
      return;
    }
    if (erasing) {
      if (erasing.length) histPush({ kind: "del", items: erasing.sort((a, b) => a.i - b.i) });
      erasing = null;
      return;
    }
    if (style.tool === "crop") return; // se confirma con los botones
    if (current) {
      const big =
        current.type === "pen"
          ? current.points.length > 1
          : Math.hypot(current.x2 - current.x1, current.y2 - current.y1) > 3;
      if (big) commit(current);
    }
    current = null;
    renderAnno();
  }

  // ---------- texto ----------
  function openTextInput(p, e) {
    closeTextInput();
    textEl = document.createElement("textarea");
    textEl.className = "text-input";
    const rect = annoCanvas.getBoundingClientRect();
    const scale = rect.width / annoCanvas.width;
    textEl.style.left = e.clientX + "px";
    textEl.style.top = e.clientY + "px";
    textEl.style.color = style.color;
    textEl.style.fontSize = Math.max(12, style.width * 5) * scale + "px";
    document.body.appendChild(textEl);
    textEl.focus();
    const size = Math.max(12, style.width * 5);
    const commitText = () => {
      if (!textEl) return; // ya cerrado (p. ej. blur tras Escape)
      const v = textEl.value.trim();
      if (v && !textCancelled) commit({ type: "text", x: p.x, y: p.y, text: v, color: style.color, size });
      closeTextInput();
    };
    textEl.addEventListener("blur", commitText);
    textEl.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter" && !ev.shiftKey) { ev.preventDefault(); commitText(); }
      if (ev.key === "Escape") { textCancelled = true; closeTextInput(); }
    });
  }
  function closeTextInput() {
    if (textEl) { textEl.remove(); textEl = null; textCancelled = false; }
  }

  // ---------- crop ----------
  function applyCrop() {
    if (!cropRect || cropRect.w < 4 || cropRect.h < 4) { cancelCrop(); return; }
    const { x, y, w, h } = cropRect;
    const tmp = document.createElement("canvas");
    tmp.width = Math.round(w); tmp.height = Math.round(h);
    const t = tmp.getContext("2d");
    t.drawImage(baseCanvas, x, y, w, h, 0, 0, w, h);
    t.drawImage(annoCanvas, x, y, w, h, 0, 0, w, h);
    setSize(tmp.width, tmp.height);
    baseCtx.drawImage(tmp, 0, 0);
    resetEditState();
    cropActions.classList.add("hidden");
    renderAnno();
    onChange();
  }
  function cancelCrop() {
    cropRect = null;
    cropActions.classList.add("hidden");
    renderAnno();
  }

  function resetEditState() {
    shapes = []; history = []; hIndex = 0;
    current = null; cropRect = null; selected = null;
    moving = resizing = erasing = null;
  }

  // ---------- API pública ----------
  let changeCb = () => {};

  function load(pngBase64) {
    const img = new Image();
    img.onload = () => {
      setSize(img.naturalWidth, img.naturalHeight);
      baseCtx.drawImage(img, 0, 0);
      resetEditState();
      cropActions.classList.add("hidden");
      renderAnno();
      onChange();
      notifyContext();
    };
    img.src = "data:image/png;base64," + pngBase64;
  }

  function flattenedPng() {
    const tmp = document.createElement("canvas");
    tmp.width = baseCanvas.width; tmp.height = baseCanvas.height;
    const t = tmp.getContext("2d");
    t.drawImage(baseCanvas, 0, 0);
    t.drawImage(annoCanvas, 0, 0);
    return tmp.toDataURL("image/png").split(",")[1];
  }
  function basePng() {
    return baseCanvas.toDataURL("image/png").split(",")[1];
  }

  function onChange() { changeCb(); }
  function setStyle(k, v) {
    style[k] = v;
    // El relleno sigue al trazo hasta que el usuario elija un color de relleno.
    if (k === "color" && !fillColorTouched) {
      style.fillColor = v;
      notifyContext();
    }
  }
  function getStyle() { return style; }
  function size() { return { w: baseCanvas.width, h: baseCanvas.height }; }

  // Cursor de borrador: mismo trazado que el icono de la barra, con halo blanco
  // para que se vea sobre cualquier fondo. Punto activo en la punta inferior.
  const ERASER_PATHS =
    `<path d='m7 21-4.3-4.3a2.4 2.4 0 0 1 0-3.4l9.6-9.6a2.4 2.4 0 0 1 3.4 0l5.6 5.6a2.4 2.4 0 0 1 0 3.4L13 21'/>` +
    `<path d='M22 21H7'/><path d='m5 11 9 9'/>`;
  const ERASER_CURSOR =
    `url("data:image/svg+xml;utf8,` +
    `<svg xmlns='http://www.w3.org/2000/svg' width='32' height='32' viewBox='0 0 24 24' ` +
    `fill='none' stroke-linecap='round' stroke-linejoin='round'>` +
    `<g stroke='white' stroke-width='3.4'>${ERASER_PATHS}</g>` +
    `<g stroke='%23222' stroke-width='1.7'>${ERASER_PATHS}</g>` +
    `</svg>") 5 27, auto`;
  const TOOL_CURSORS = { cursor: "default", text: "text", eraser: ERASER_CURSOR };
  function setTool(t) {
    style.tool = t;
    selected = null;
    annoCanvas.style.cursor = TOOL_CURSORS[t] || "crosshair";
    renderAnno();
    notifyContext();
  }

  // ---------- contexto (popover de relleno) ----------
  let contextCb = () => {};
  function currentContext() {
    if (style.tool === "rect" || style.tool === "ellipse") {
      return { showFill: true, anchor: style.tool, fillMode: style.fillMode, fillColor: style.fillColor };
    }
    if (style.tool === "cursor" && selected && (selected.type === "rect" || selected.type === "ellipse")) {
      return {
        showFill: true, anchor: selected.type,
        fillMode: selected.fillMode || "none", fillColor: selected.fillColor || style.fillColor,
      };
    }
    return { showFill: false };
  }
  function notifyContext() { contextCb(currentContext()); }

  function selectedFillable() {
    return style.tool === "cursor" && selected && (selected.type === "rect" || selected.type === "ellipse")
      ? selected : null;
  }

  // Cambia el modo de relleno (ninguno / color del trazo / otro color).
  function applyFill(mode) {
    style.fillMode = mode;
    const s = selectedFillable();
    if (s) {
      s.fillMode = mode;
      if (mode === "other") s.fillColor = style.fillColor;
      renderAnno();
    }
    notifyContext();
  }

  // El usuario elige un color de relleno explícito → deja de seguir al trazo.
  function setFillColor(color) {
    fillColorTouched = true;
    style.fillColor = color;
    style.fillMode = "other";
    const s = selectedFillable();
    if (s) { s.fillColor = color; s.fillMode = "other"; renderAnno(); }
    notifyContext();
  }

  // eventos de puntero
  annoCanvas.addEventListener("pointerdown", onDown);
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  document.getElementById("crop-apply").onclick = applyCrop;
  document.getElementById("crop-cancel").onclick = cancelCrop;

  return {
    load, flattenedPng, basePng, undo, redo, clear, applyCrop, cancelCrop,
    setStyle, getStyle, hasImage, size, setTool, deleteSelected,
    zoomIn, zoomOut, fitView, applyFill, setFillColor,
    onChange: (cb) => (changeCb = cb),
    onView: (cb) => { viewCb = cb; applyView(); },
    onContext: (cb) => { contextCb = cb; notifyContext(); },
    show: () => wrap.classList.remove("hidden"),
  };
})();
