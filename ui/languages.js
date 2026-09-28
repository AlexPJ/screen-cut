// Nombres en español de los idiomas de Whisper y relleno de los <select> de
// idioma (selector de sesión, Ajustes y visor).
window.Languages = (() => {
  let names = null;
  try { names = new Intl.DisplayNames(["es"], { type: "language" }); } catch {}
  const COMMON = ["es", "en", "ca", "gl", "eu", "pt", "fr", "de", "it", "ar", "ro", "zh"];

  function name(code) {
    if (code === "auto") return "Autodetectar";
    const n = names && names.of(code);
    return n && n !== code ? n[0].toUpperCase() + n.slice(1) : code;
  }

  // "Autodetectar", los habituales y el resto por orden alfabético.
  function fill(select, languages, value) {
    select.innerHTML = "";
    const add = (parent, code) => {
      const o = document.createElement("option");
      o.value = code;
      o.textContent = name(code);
      parent.appendChild(o);
    };
    add(select, "auto");
    const codes = languages.map((l) => l.code);
    const common = document.createElement("optgroup");
    common.label = "Habituales";
    COMMON.filter((c) => codes.includes(c)).forEach((c) => add(common, c));
    const rest = document.createElement("optgroup");
    rest.label = "Todos";
    codes
      .filter((c) => !COMMON.includes(c))
      .sort((a, b) => name(a).localeCompare(name(b), "es"))
      .forEach((c) => add(rest, c));
    select.append(common, rest);
    select.value = codes.includes(value) ? value : "auto";
  }

  return { name, fill };
})();
